# PrepareからCommit票保存までの待機短縮

## 目的と固定条件

安全に使える無償OSS、小規模サイトでの試験導入が目標。今回の範囲は必須保存の長い停止の切り分けと、保存後の不要な待機の最小削減。WAL方式、file/parent sync_all、TTL、NoOp 2秒等の既存期限、定足数、署名前の永続化条件を維持する。

証拠: `artifacts/phase-wait-m6tvcqft/`、pi-nodeの専用作業コピー `/home/maakie/veil-censor-47iX3Rbs/`。実行バイナリは修正前 `tests-prepare-final`、修正後 `tests-phase-final` と分離しhashを保存した。

## OS側で確認したこと

pi-nodeはLinux 7.0.0-38-generic、x86_64。対象WALの置き場所は `/dev/sda5`、ext4、rw/relatime。対象デバイスはSATA接続のEAGET SSD 1TB、I/O schedulerはmq-deadlineだった。Raspberry Pi/ARMという前提は用いない。

自身が起動した試験プロセスに限定してstraceでfsync/fdatasyncの時刻・所要時間を取得し、その子プロセスの各threadのstate/wchanとホストの集計pressure/diskstatsを100ms間隔でメモリに記録した。秘密鍵・通信本文・ファイル本文はsyscall trace対象にしていない。root、NTP、mount設定、I/O schedulerの変更は行っていない。

修正前の観測2回はいずれも単独検閲試験に成功したが、1回目にfsync 860.402ms、その直後の別fsync 274.526msを観測した。860msの呼出しと同じTID=1844501、同じ時刻区間のサンプルは、最初がD/folio_wait_bit_common、その後8回がD/jbd2_log_wait_commitだった。少なくともこの停止は、Tokioの投入待ちだけでなく実際にfsync内で生じている。

[Linux公式のext4 journal資料](https://docs.kernel.org/filesystems/ext4/journal.html)も、ジャーナルへのトランザクション書込み・flush・commitを説明している。ただし、待ち先の観測からデバイス内部の遅延、他の書込みとの競合、ファームウェア等のどれが根本原因かは分からない。ext4の特定のロック競合やSSD故障と断定しない。2回目の最大fsyncは57.560msだった。ptraceと詳細ログの影響もあるため、この2回を無計測実行との性能比較には使わない。

この860msの停止は交代中の時刻であり、以前のNoOp 2秒失敗そのものを同一実行でカーネル原因まで特定したわけではない。

## 削除したアプリ側の待機

従来のdriverは並列RPC roundの全応答／期限終了を待ち、次の200ms tickで次段階を作っていた。Prepare定足数が先に揃い、prepare証明と自分のCommit票を保存済みでも、遅い相手の終了までPrepared送信を開始できなかった。

`exchange_round` は既存の署名検証・handle_signed_rpc・checkpointが成功した後、leaderのprepare証明位置またはcommit位置の進行を検知し、その場で各peer向けの既存make_append_entries/sign_rpc_durableを呼ぶ。他peerの未完了RPCは残したまま、次の送信を開始する。通常の200ms tick、800ms round内RPC期限、QUICの既存期限は変更しない。

追加送信は1 roundにつき最大2回のpeer集合への送信に制限する。通常はPreparedとCommitの2段階。既存のpeerごとのnext_index/certified catch-upを使い、必要な親commitを飛ばさない。古いtermの票・集約済みの重複票・無効票、役割喪失、保存失敗で追加送信を開始しない。無限の即時再試行や永続化前の投票解放にはしない。

この変更はfsyncそのものを短縮するものではなく、必須保存の後にさらに乗っていたフェーズ間待機を削除するものである。追加送信により瞬間的な通信・検証負荷が増える可能性があるため、単独だけでなく並行回帰も採否に含める。

## 順序固定回帰

- `durable_quorum_advances_phases_before_the_slow_peer_finishes`: 4台中1台のtransportを応答しない状態に固定。修正前は遅いpeerの期限を迎えてroundが終わるまでPreparedを送れず失敗。修正後は遅いpeerが終わる前に他2台へPreparedを届け、Commitまで完了する。送信数は初期3＋Prepared3＋Commit3の9回で上限も確認。これは署名付き状態機械と模擬transportの試験であり、NICや実ストレージ性能の試験ではない。
- `failed_prepare_save_never_dispatches_phase_followups`: leaderの実FileWalへ成功保存した後、prepare証明形成時の保存失敗を注入。初期3送信だけで追加送信0、persistence_errorと元のWAL bytes保全を確認する。

既存のView/投票同時保存、保存前署名禁止、取消中workerの排他、競合QC、期限不変、WAL復元等の回帰も実行する。

## 実機比較・回帰結果

詳細ログ／straceなしの単独試験をbefore/afterで3組交互に実行した。修正前後とも2成功・1失敗であり、タイムアウト解消・成功率向上は証明されていない。

修正後の失敗はNoOp 3、leader a、term=1、wait=4,675〜6,676ms。leaderのPrepare票は自分だけで、フォロワーb/c/dは4,851/4,851/4,857msからのPrepare票保存が期限時点で未完了だった。今回の即時送信条件である定足数成立まで到達しておらず、削除したフェーズ間待機とは区別する。

| 回帰 | 結果 |
| --- | --- |
| macOS workspace、2スレッド | 174成功、0失敗、1除外、44.63秒 |
| pi-node修正後、並行1回目 | **173成功、2失敗**、6除外、3フィルター除外、57.00秒 |
| pi-node修正前、追加並行比較 | 173成功、0失敗、6除外、3フィルター除外、49.27秒 |
| pi-node修正後、追加並行比較 | 175成功、0失敗、6除外、3フィルター除外、48.29秒 |
| pi-node実XDP split recovery | 成功、12.50秒。Drop→期限失効Pass→XDP解除・隔離ping成功 |
| macOS/Linux Clippy、format/diff | 成功 |

並行1回目の失敗を追加成功で取り消さない。内訳は初期選挙＋NoOpの15秒期限、検閲試験のNoOp 5の2秒期限。後者ではb/c/dのPrepare票保存に1,748〜1,758msかかっている。初期選挙の失敗時はlog=0/commit=0、term=4、cへの投票が見え、一部stateがbusyだった。その失敗の原因を今回の追加送信と同一と断定できず、追加比較だけで無関係と確定したとも扱わない。B3の通常leader停止後の交代試験は実行しておらず、この初期選挙失敗とも区別する。

変更はdriverの有界な即時フェーズ送信と、状態機械の非永続な位置参照、2件の回帰のみ。WAL、state_worker、QUIC、policy_clock、daemon_tests、consensus_engine、progress、electionの8ファイルは作業前hashと一致した。TLS更新後3秒待ちや小規模サイト試験導入の合格判定には流用しない。

**判定：フェーズ間待機の削減は局所回帰で確認。保存停止自体と単独・並行回帰の安定合格は未達。** 変更は未コミットの作業成果として保持するが、運用採用は保留する。保存が未完了なら票を返さない条件を維持したため、長い保存停止を隠して合格にする変更はない。次に残るのは、必須fsync内の待ちの原因・上限と、このホストで既存期限を安定して満たせるかの判定である。

## 最終の段階ログ確認

修正後のstrace付き単独試験は28.85秒で成功、fsync 272回、最大45.623msだった。NoOp index 3〜7について「異なる2 peerからのproposal RPC応答到着の2番目→最初のPrepared送信」を照合した結果は次の通り。

| index | 修正前の観測2回目 (ms) | 修正後の最終観測 (ms) |
| --- | ---: | ---: |
| 3 | 68.737 | 33.746 |
| 4 | 44.878 | 107.036 |
| 5 | 54.021 | 48.745 |
| 6 | 43.408 | 45.204 |
| 7 | 74.875 | 41.840 |

これはRPC応答到着からの区間であり、署名検証、leader自身のprepare証明・Commit票保存、worker待ち等も含む。永続化完了から送信までだけの値ではない。別実行でOS負荷も一致しておらず、全区間の一律短縮や改善率を主張できない。削除した待機の因果確認は、遅いpeerの終了前にPreparedが届く順序固定回帰による。詳細は証拠ディレクトリの `final-trace-summary.json` と各trace/logに保存した。
