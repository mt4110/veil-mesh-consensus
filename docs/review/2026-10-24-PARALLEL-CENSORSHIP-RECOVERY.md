# 並行回帰での検閲リーダー交代タイムアウト

2026-10-24。対象は通常daemonの実行方式。署名context 6、WAL v9、prepare/commit、定足数、投票・ロック・checkpoint形式は変更しない。

後続の時計運用修正と、TLS更新後の失敗を段階ログで再現した結果は [時計品質とTLS待ちの調査](2026-10-24-CLOCK-QUALITY-AND-TLS-LATENCY.md) を参照。以下の失敗記録は当時の証跡として残す。

## 原因と再現

pi-nodeの2スレッド並行回帰で、検閲リーダー試験を診断付きで再現した。停止位置は18秒のリーダー交代待ちではなく、その前に「検閲リーダーも無関係な合意は進めている」ことを確認するNoOp 4の確定待ち（2秒）だった。daemon試験4件だけの2スレッド実行でも再現した。旧コードの全体回帰が通る実行もあり、常時発生する障害ではなかった。

同じ記録にはWALの同期永続化が937、948、971、1006 ms掛かる場面がある。旧コードは `FileWal::persist` のwrite / file sync / rename / directory syncを、Tokioの非同期タスクから直接呼んでいた。試験はcurrent-thread runtime上に4ノードを置くため、1ノードの同期I/Oが他の3ノードの受信・送信・タイマーまで止める。通常daemonでも非同期workerを占有する実装であり、実行スレッドを増やして試験だけ回避する対応にはしない。

これは観測したソフトウェア上のブロッキングの原因である。ホスト側のsync遅延がSSD・仮想化・他プロセスのどれに起因するかは特定していない。WAL破損や合意の安全性違反を観測したという意味ではない。

さらに、executorの分離だけではNoOp 3の2秒待ちが再度失敗した。その記録も保全した。追加観測ではsyncに約1.7秒、ホストのI/O pressureにも待ちが見られた。コードでは、prepare定足数成立後に4票目を集めると、既存の有効な証明をより大きい証明に置き換えて再同期する経路を確認した。実WALの計数回帰で書き込み5回→6回を再現した。

## 修正と安全性

`infra::state_worker::run` は、ノードの状態ロックを非同期に取得した後、同期状態遷移全体を `spawn_blocking` へ移す。通常daemonのtick、RPC受信・応答処理、ローカル操作、適用再試行がこの経路を使う。1ノードのWALが遅くても、他ノードやネットワークの非同期executorを占有しない。

- WALを同期してから投票・署名済み応答を返す順序を維持する。syncの省略、書き込みの先送り、投票の先行送信はしない。
- 状態ロックはWAL・状態更新・適用処理の完了まで保持する。同一ノードの複数workerを並列実行しない。ロック待ちをblocking poolへ積まない。
- 呼び出し元のtimeout/cancel後も、開始済みの遷移はロックを保持して完了する。応答を失った操作は受付済みの可能性があり、従来どおり状態照会が必要。
- Tokioのmutexはpanicでpoisonされないため、worker内のunwindを捕捉し、永続化エラーをラッチして以後の投票を停止する。再起動・WAL復元なしに続行しない。panic=abortやOS強制終了では従来のクラッシュ復旧に従う。
- ディスク停止自体は解消しない。同じノードの処理は安全のため待つ。開始済みblocking処理は強制キャンセルできず、runtime終了もその完了を待つ。任意のストレージ障害で進行時間を保証する変更ではない。

prepare定足数と自ノードのcommit投票フェンスが同じview・高さ・blockについて既に存在するとき、追加の検証済みprepare票で証明を書き換えない。入力署名の検証は省略せず、初回のprepare証明・commitフェンスの同期は維持する。少なくとも定足数分の署名を含む元の証明が残るため、証明強度は変わらない。これにより証明の拡張に伴うリーダーとfollowerの重複同期を避ける。

NoOp確定2秒、交代18秒、BAN期限、RPC制限、選挙条件、検閲判定の猶予、交代前のfresh応答・無関係な確定のassertは維持した。テストfixtureの提案・適用も通常daemonと同じworker経路を使う。失敗した待ち段階をエラーに付け、任意の `VEIL_TEST_TRACE=1` で状態・WAL遅延を記録できるようにした。

## 検証

- current-thread runtimeでWALをゲート停止し、別ノードの進行、同期完了前の応答保留、排他保持を確認した。旧来のexecutor上で直接実行する方式へ戻した隔離コピーでは、この検査が意図したassertで失敗した。修正コードでは通過した。
- 呼び出し元のキャンセル後も遷移を排他したまま完了すること、同期失敗とworker panic後に投票を再開しないことを確認した。
- 追加prepare票の実WAL書き込み回帰は修正前に5→6回で失敗し、修正後は回数・WAL bytesとも不変。WAL復元後の証明検証と別リーダーによる確定も通過した。
- macOS全体回帰: 160 passed / 0 failed / 1 ignored。fmt、clippyも通過した。

- pi-nodeの2スレッド全体回帰: 両修正後に160 passedの実行を2回得たが、3回目はNoOp 4の2秒待ちで159 passed / 1 failed。**この時点では間欠タイムアウトは未解消**。計測付きの追加3回でも160 passedが2回、NoOp 4の期限超過が1回となった。
- pi-nodeのclippy・build、root実カーネル3件（TTL/SIGSTOP・bypass・WAL復旧、遅延catch-upの署名期限、競合prefixからの自動交代とXDP適用・失効）が通過した。
- 起動black-box 9条件（WAL欠損・空・破損、同じ鍵での再初期化拒否、保全WALからの復旧等）が通過した。
- 通常daemon 4プロセスで、選挙・承認BAN・実XDP適用・失効・リーダー停止と再起動・定足数喪失・TLS更新・個別解除・再送拒否・checkpoint・認証付き状態転送が通過した。試験内のXDP除去・隔離pingも成功した。BPF、ネットワークfixture、適用範囲は前回と同じで、物理NIC・UFW・NTPは変更していない。

非公開証跡は `artifacts/censorship-0a_jyj2s/`、Linux側は `/home/maakie/vcp-xiPjYQ/`。前作業からの未コミット変更を保全し、今回差分は `task.patch` で分離する。

- I/O段階observer導入後の2スレッド回帰は160 passedを3回確認。その後の終了時統計付き3回では、検閲試験は全回通過したが、3回目に `authenticated_quic_rejects_legacy_and_zero_bans_but_accepts_finite_and_unban` のRPCが期限超過し、全体は159 passed / 1 failed。診断変更しか加えていないため、成功の反復だけで以前の間欠障害を解消扱いにしない。

## 待ち時間の追加観測

並行回帰の失敗時に、待ち開始5842 ms・期限7843 msに対して、follower aのWALが6187〜7088 ms（901 ms）、bが6191〜7097 ms（906 ms）、cが6188〜7106 ms（918 ms）、leader dは7217 msから保存中のまま期限に達した。これは同じfixtureの単調時計で計測した値であり、別試験のnode名を混同していない。

失敗は2秒のTokio待ちで発生した。このfixtureはMockEbpfBridgeを使用し、Aya/eBPFをロードしない。WALのI/Oがカーネルを経由することと、タイムアウトを検出した層は別である。第三者監視やセキュリティ製品の介入を示す証拠はない。

Rust側のFileWalには、writer lock、検証、作成、write、file sync、rename、directory open、directory syncの境界observerを追加した。既定経路のobserverは空処理であり、同じI/O・同じsync順序を実行する。試験側は有界の時刻履歴を保持し、失敗時に出力する。`VEIL_TEST_WAL_STATS=1` では、完了した試験の100 ms以上の区間も表示する。通常daemonも `RUST_LOG=info,veil_wal_timing=debug,veil_apply_timing=debug` を指定すると、WAL段階と適用の開始・完了を記録する。WAL内容や秘密鍵は記録しない。

段階別計測では、成功した検閲試験中にfile sync 105〜187 ms、directory sync 121 msを観測した。これらを、それ以前に観測した約1秒の停止箇所が確定した証拠とは扱わない。pi-nodeのデータ領域はSATA SSD上のext4。直近40分を対象に読んだストレージ関連kernel journalには該当エラーがなく、SMART総合判定はpassedだった。ハードウェア故障や外部プロセスの影響を完全に排除する検査ではない。

最終ソースの実カーネル3件と起動black-box 9条件は再度通過した。ただし、通常daemonの再実行 `flow-stages` はTLS更新後のBAN適用待ち（3秒）で失敗した。初回 `flow-final` の10段階成功を残しつつ、最終再試験を成功扱いにしない。後片付けはXDPなし・隔離ping成功。遅延の原因が先のNoOpと同一かは未確定である。

同じ最終ソースの2スレッド回帰を、試験プロセスだけの `strace -f -ttt -T -y -e trace=fsync,fdatasync,rename,renameat,renameat2` で観測した実行は160 passed。この実行のfsyncは1,313回、最大114.783 ms、300 ms超は0回。renameは504回、最大3.965 msだった。時間はシステムコール区間の経過時間であり、CPU実行時間ではない。トレースの影響を含むため性能保証には使わず、過去の約1秒の停止箇所を確定したとは扱わない。対象試験以外のプロセスへattachしていない。

## TLS更新後の3秒待ちの判定と診断

この待ちは全ノードのentry hash・commit_index・last_applied・エラー状態に加えて、ローカル制御ソケット経由の `policy_mirror=Drop` を要求する。物理NICのRTTやBPF map更新時間ではない。合意通信は同一ホストの127.0.0.1上のQUIC/UDPで、実XDPはnode dの隔離vethだけで動く。

対象BANのTTLは4秒だが、承認CLIは発行時刻を2秒戻し、期限はその時刻から計算するため、署名直後の残り期限は約2秒。さらに安全な時計誤差評価で実効期間は短くなり得る。従来の失敗ログには全ノードの途中状態が残らず、未確定・未適用・適用後失効を区別できない。元の失敗を「TLS通信が3秒掛かった」とは解釈できない。

診断として、black-box試験は適用待ちの直近32サンプルについて要求ID、index、署名期限、残り時間、照会所要時間、各ノード状態を保持し、期限超過時にresult.jsonへ保存する。待ち時間・TTL・判定条件は変更していない。

`flow-timing` の診断付き再試験は初期BAN・失効とfollower経由の承認まで通過し、リーダー停止後のBAN承認で `Clock bound required for operator approval` により拒否された。TLS更新段階には未到達。後続のread-only確認ではNTP同期表示はyesだったが、その表示だけではアプリが要求するadjtimexの誤差上限を満たす証拠にならない。NTPや許容誤差の設定は変更していない。

この実行のWAL計測97回でfile sync最大42.018 ms、directory sync最大152.987 ms。適用完了17件の区間最大40.386 ms（適用内部のWAL処理を含み、BPF map syscall単独ではない）。ログを有効にした実行に限る観測値であり、元のTLS待ち失敗の原因を確定する値ではない。pi-node build/clippy、macOS state-worker回帰3件、fmt/diff検査は通過。隔離XDPの除去・試験パケット疎通の復旧も確認した。

## 完了範囲

**依頼全体は未完了。** 非同期executorの同期I/O停止と、余分なprepare証明更新は修正した。回帰の成功・再発の両方を確認しており、並行回帰の安定合格と通常daemonの進行時間は未達。長い停止が起きた同じ実行で、追加した段階ログとsyscallログを対応させることが残る。2秒・300ms・3秒の期限や、投票前のWAL同期を緩めて通過させない。

この修正は並行時の非同期executor停止とその回帰を対象とする。小規模サイトへの試験導入全体、複数ホストの耐障害性、長時間負荷、運用者鍵の復旧、未保護プロトコルへの対応を完了とするものではない。

## 追加検証：検閲単独試験の失敗段階と同一term再送

WAL区間計測とfsync/fdatasync比較の後、検閲単独試験に対象を絞って再検証した。B3の `replacement leader with 3 of 4` は通常プロセス停止後の選挙であり、ここでの修正成功をその修正証拠に流用しない。

証拠: ローカル `artifacts/censor-stage-e3xhy511/`、pi-node `/home/maakie/veil-censor-47iX3Rbs/`。今回のLinux buildは固有のtargetに分離し、CargoのJSON出力にある実行ファイルを各段階で別名保存した。既存の未コミット作業と失敗ログは保全した。

### 最初に観測した失敗

計測・エラー文脈だけを追加した単独3回は1成功・2失敗だった。

- `diagnostic-2.log`: NoOp index=3の2秒待ちが失敗。leader cはterm=1、commit=2、prepare票は自分だけ。他3ノードは状態処理中。aのfile sync区間1,863ms、bのcreate区間1,845ms、dのvalidate区間1,849ms。検閲交代待ちに入る前の、保存待ちを伴う失敗である。各値は呼出し区間の経過時間であり、カーネル内部の原因を特定した値ではない。
- `diagnostic-3.log`: NoOp index=6の待ちで失敗。旧leader dはterm=1、commit=5、prepare票はa/c/d、commit票は自分だけ。一方a/b/cはterm=2へ移行済み。約0.9秒の保存区間もあった。旧leaderの未完了NoOpと、正当な検閲証明に基づく交代が競合するケースとして区別する。
- その後、診断を補った版の単独3回は全成功。失敗が常に再現する決定論的デッドロックとは判定しない。

初期選挙、初期BAN受付・全台適用、検閲要求受付、NoOp、交代後全台適用、期限失効をエラー文脈で区別するようにした。失敗時はtry_lockで各ノードのterm/role/leader、commit/applied、prepare/commitの投票者、prepare証明の位置を出す。取得できない状態を待ち続けずbusyとして記録する。秘密鍵・署名本文は出力しない。

### 修正1：候補者のView要求を同一termで再送

`drive_two_phase` はleader以外でNoneを返し、候補者が最初のViewRequestへの応答を失っても、同じtermの要求を再送していなかった。次の選挙機会に頼る経路であり、永久停止とまでは主張しない。

対象BANの検閲証明による移行後、候補者が2票を持ち、3台目が報告を永続化した後の応答だけを失う順序を固定した。`censorship_candidate_retries_lost_view_report_without_a_new_term_or_vote` は修正前に「同じtermで再送されない」として失敗、修正後に成功した。

変更は、candidateで、自己投票と同じterm/候補者の凍結済みreportがあり、除外されていない場合に、既存driver tickで同じViewRequestを `sign_rpc_durable` 経由で返すことだけ。termを上げず、集約中reportを消さず、相手の凍結済みreportも変更しない。既存の全peer配信、受理検証、定足数、同期前の署名解放禁止を使う。試験でcandidate/応答者のWAL bytesが再送前後で不変、再送されたreportが失われたものと同一、同じtermで定足数成立を確認した。

これは確定した再送漏れの修正であり、前述のNoOp保存待ちの原因と同一だとは主張しない。

### 修正2：正当な早期交代を試験が誤判定しない

再送修正後の単独試験では、NoOp index=5で約1.45秒の保存区間を伴う2秒待ち失敗に加え、5件すべてのNoOp完了後に `current_term == old_term` のassertionが失敗する回を確認した（left=2、right=1）。後者は検閲監視の5秒猶予を超えてから旧termを要求する試験の誤りである。

5件のNoOp・各2秒の条件は保持する。その進行中に、旧termでfresh応答によるactivity増加と無関係なcommit進行を実際に観測したことを記録し、必須assertionとした。後からtermが進んでいた場合は、元の要求ID・旧term・旧leaderに一致する有効なProgressCertificateを検証する。queued/appliedはその証拠を持つ交代後だけ許容し、その後の新leader・全honest nodeのDrop、元の署名期限の不変、旧leaderの残存、期限失効の検査も残す。

`certified_censorship_handoff_can_interrupt_an_unrelated_old_leader_noop` は旧NoOp途中の交代を順序固定で再現する。旧termの追加投票を拒否し、正当な新leaderで対象BANを元のcommandのままcommitできることを確認する。これは修正前から成立する安全な挙動であり、新しいプロトコル改善と数えない。要求ID・term・leaderが異なる証拠は試験の早期交代条件を満たさないことも確認する。

### 固定した条件と残る問題

WAL形式、sync_all、状態workerの排他と永続化順序、TTL、NoOp各2秒、交代18秒、全honest node適用4秒、失効30秒、RPC期限、検閲猶予5秒、選挙タイマー、定足数を変更していない。B3を含む通常daemon全工程、TLS後3秒の再比較は今回の検証対象にしていない。NTP・ホスト設定も変更していない。

試験の早期交代検査を修正しても、旧leaderのNoOpが2秒に間に合わない失敗は成功へ置き換えない。保存停止が続く環境で安定合格を達成したかは、以下の最終検証結果で別に判定する。

### 最終検証結果

- macOS最終workspace回帰: 166成功、0失敗、1除外（2スレッド）。単独検閲試験も成功。
- pi-node最終単独検閲試験: **3回中2成功・1失敗**。`final-1.log` はNoOp index=6の2秒待ちで失敗。leader dはterm=1、commit=5、prepare票は自分だけで、他3ノードはbusy。待ち期間にfile sync 673〜686ms、directory sync 381msを記録し、期限時点で次のfile sync区間も未完了だった。早期交代を誤判定した失敗ではない。
- pi-node最終並行回帰: 167成功、0失敗、6除外、3フィルター除外（2スレッド）。フィルター除外は従来と同じroot/TUI用試験。単独失敗は、この並行回帰成功で取り消さない。
- pi-node実XDP: `linux_two_phase_split_recovery_applies_and_expires_in_real_xdp` 成功。通常driver・TLS/QUIC・Commit QC・隔離vethの実Drop・署名期限のPass復帰を確認。XDPなし・隔離ping成功も確認した。これは検閲故障の実XDP試験ではなく、共有する交代経路の安全性回帰である。
- format/diff検査とmacOS/Linux Clippy成功。

WAL/state_worker/QUIC/daemonタイマー/policy_clockのソースhashは作業前と一致。fixture内のDuration定数列とNoOp5件条件も一致する。変更は製品の候補者再送1分岐、テスト用診断・証明照合、試験の時点誤認修正、2件の順序固定回帰である。

**今回の依頼は部分完了。** 失敗段階の特定、再送漏れと試験誤判定の最小修正、並行回帰・pi-nodeでの再検証は実施した。ただし元の単独試験の安定合格は未達。長い保存待ちを伴うNoOp2秒条件の失敗は残件とする。5件NoOp途中で正当な交代が先行し、旧leaderの残りNoOpをcommitできなくなる場合も、現行条件では失敗として残る。条件緩和で合格化していない。

次の作業は保存方式を固定したまま、失敗NoOpに対する各保存の意味・RPC応答喪失・再試行を対応づけ、必要な永続化と不要な待機を区別すること。今回の結果だけで安全に省略できる同期や、新たに緩めてよい期限が見つかったとはしない。B3、TLS後3秒、小規模試験導入の合格判定は引き続き未達/未検証の範囲を維持する。

## 継続検証：NoOp保存の意味と応答喪失の切り分け

証拠: `artifacts/censor-noop-nyvxq8wu/`。pi-nodeは前回の専用作業コピー `/home/maakie/veil-censor-47iX3Rbs/` を再利用し、実行ファイルを `tests-noop`、最終回帰用を `tests-noop-final` と別名保存した。前回までのバイナリとログは保全した。

### 変更と再現

今回は製品の合意・保存ロジックを変更していない。前回の同一term ViewRequest再送修正は維持した。追加したのは以下の試験用コードである。

- `phase_snapshot_diagnostics`: これから保存するv9 snapshotからterm、末尾index、commit index、Prepare/Commit投票位置、prepare証明位置だけを取り出す。コマンド、署名、鍵、snapshot本文は出力しない。テスト時のみコンパイルする。
- `TimedWal`: 保存開始・終了時刻に上記ラベルを関連付ける。記録は既存の128件上限のメモリ内に保持し、失敗時のみ出力する。状態ロックがbusyでも、何を保存中かを確認できる。
- `lost_prepare_and_commit_replies_retry_without_rewriting_durable_votes`: PrepareとCommitそれぞれで、2フォロワーの保存が済んだ後の応答を捨て、同じ要求を再送する。実FileWalからの復元に投票履歴が残ること、再応答が同一であること、再送で保存回数・WAL bytesが変わらないこと、Commit QCを受けるまでフォロワーはcommitしないこと、再送した票で合意・適用が完了することを確認する。ネットワーク遅延の実測ではなく、順序を固定した応答喪失回帰である。

区間詳細ログありの既存バイナリ単独実行は成功（29.50秒）。新しいメモリ内ラベル付き・詳細ログなしの単独3回は **2成功・1失敗**。`noop-1.log` の失敗を以下の通り切り分けた。

| 対象 | 失敗時に確認した事実 |
| --- | --- |
| NoOp | index=6、旧leader=b、term=2。待機13,127〜15,128ms、既存2秒期限で失敗 |
| leader b | commit=5、applied=5、Prepare票はbのみ。prepare証明はindex=5、persistence_errorなし |
| follower a | index=5のcommit保存は13,222msに完了。index=6のPrepare票保存は13,357msに開始、13,358msのfile-sync開始後、期限時点で未完了（区間の観測下限1,770ms） |
| follower c | index=5のcommit保存は13,211msに完了。index=6のPrepare票保存は13,380msに開始、create開始後に未完了（観測下限1,748ms） |
| follower d | index=5のcommit保存は13,210msに完了。index=6のPrepare票保存は13,369msに開始、validate開始後に未完了（観測下限1,759ms） |

3フォロワーとも、保存しようとしている状態はterm=2、log=6、commit=5、Prepare投票=(2,6)、Commit投票=(2,5)、prepare証明=(2,5)だった。つまりこの失敗は、新Viewの報告集約や交代後BAN適用の段階に到達する前の、無関係NoOpのPrepare票永続化待ちである。状態mutexがbusyであることだけからデッドロックとは判断しない。区間時間はその呼出しの経過時間であり、カーネル内部での競合箇所やSSDそのものの故障を特定するものではない。

保存完了前に票を返すこと、2秒を延ばすこと、失敗を成功へ読み替えることは行っていない。応答喪失後の再送は順序固定回帰で正常と確認したが、失敗回の全RPC応答・再送時刻を取得したわけではない。取得済みの実測と、固定順序テストの証拠を混同しない。

### 最終回帰と判定

- macOS workspace並行回帰（2スレッド）: **167成功、0失敗、1除外**。46.14秒。
- pi-node並行回帰（2スレッド）: **168成功、0失敗、6除外、3フィルター除外**。53.58秒。従来と同じroot/TUI系のフィルターを維持した。
- pi-node実XDP split recovery: **成功**、13.02秒。実Drop、署名期限によるPass復帰、XDP解除、隔離ping成功を確認した。検閲攻撃そのものの実XDP試験ではない。
- macOS/Linux Clippy、format/diff検査: 成功。
- 新規の応答喪失回帰はmacOS単独でも1件実行・成功し、両OSの並行回帰でも成功した。最初の`--exact`指定では0件だったため、それを検証数に含めず、フィルターを訂正して実行した。

**安定合格は未達、依頼全体は引き続き部分完了。** 何の保存で止まったかは今回確定できたが、今回新たに修正すべき合意プロトコル欠陥は確認できなかった。必要な投票永続化が期限時点で未完了の実行を、再送変更だけで安全に成功させることはできない。改善対象は保存待ちを含むクリティカルパスであり、その方式変更は今回の固定条件に含めていない。

WAL保存方式・sync_all・TTL・各時間制限・定足数・署名前の永続化条件・ホスト時計設定は不変。B3通常選挙失敗、TLS更新後3秒待ちは別件として未検証を維持する。小規模サイトでの試験導入可とは判定しない。

## Prepare保存待ちの最小削減：Viewと投票の同時保存

証拠: `artifacts/prepare-save-ecbu0t7y/`。最終ゴールは安全に使える無償OSS、マイルストーンは小規模サイトでの試験導入。今回の範囲はWAL方式・sync_all・TTL・時間制限・安全条件を固定した不要な保存の削減と、単独／並行／pi-node回帰である。

### 採用した変更

新しいNewViewを初めて受けるProposal/Prepared処理では、従来 `activate_phase_view` がViewを保存し、その後 `phase_vote` が投票履歴（Preparedの場合はprepare証明も）を別のsnapshotで保存していた。この2つの保存の間に外部へ応答を返さず、同じ状態workerの排他区間内で処理している。

`with_phase_view` でView受理と投票処理を包み、`phase_vote` の既存checkpointで両方を保存するよう変更した。投票署名は従来通り、このcheckpointの成功後にだけ生成される。途中のView単独保存を省き、新View受理時の実保存を2回から1回にする。FileWalの一時ファイル作成、write_all、file.sync_all、rename、parent.sync_allはそのままであり、WALフォーマットや署名形式も変更していない。

投票に進まない経路も省略しない。検証済みViewに対して提案が拒否された場合、あるいは既に確定済みなのでReceiptだけを返す場合は、wrapper末尾でViewを保存する。投票内の保存が失敗した場合は既存のpersistence_errorで停止し、応答を解放しない。正常投票の後のwrapper checkpointは同一snapshotとして既存のhash比較で実I/Oを省略する。

クラッシュが統合保存の完了前なら、新しい投票は外部へ出ていない。完了後なら、View、投票履歴、必要なprepare証明が同じ復元対象に含まれる。この境界を維持するため、保存をバックグラウンド化したり状態ロックを途中解放したりはしていない。

### 削減していない箇所と効果の限界

- 同じView内の新規Prepare投票は元から実保存1回。今回も1回である。以前のNoOp index=6で約1.75秒止まったこの経路から、さらに安全に削除できる保存は確認していない。
- 同一投票の再送は元から追加保存0回であり、これも維持する。
- driverの200ms tick、RPC期限、並列送信roundの待機構造は変更しない。ネットワーク待機方式の変更を今回の保存削減と混ぜない。
- 保存2回→1回という回数はテストで確認できるが、daemon全体が2倍速くなることや、単独試験の停止がなくなることは意味しない。

### 順序固定回帰

`first_view_and_phase_vote_share_one_durable_snapshot` は修正前に実保存2回で失敗、修正後はPrepare/Commit両経路で1回を確認した。実FileWalの復元結果にView・投票履歴・prepare証明が含まれることも確認する。

追加で、保存失敗時の無応答・以後の署名停止と元のWAL保全、拒否提案でも受理したViewを保存すること、投票しないReceipt経路のView永続化、同一Viewでの新規Prepare1回／重複0回を確認した。既存の旧View投票拒否、競合prefix、隠れたQC、応答喪失、期限不変の回帰も維持する。

### 実機比較と最終判定

pi-nodeでは既存の修正前binary `tests-noop-final` と、今回の `tests-prepare-final` をbefore/afterの順で3組交互に実行した。いずれも同一ホスト・同じ既存fixture・同じ環境変数・詳細ログなし・1 test thread。乱数による選挙とホストI/O変動は固定していないため、速度改善率や統計的な成功率改善の証明には用いない。バイナリhash、11ファイルのlocal/remoteソースhashを証拠に保存した。

| 検証 | 結果 |
| --- | --- |
| macOSのView関連回帰 | 14成功、0失敗 |
| pi-nodeのView関連回帰 | 14成功、0失敗 |
| pi-node単独・修正前 | 1成功、2失敗（NoOp 7、NoOp 3の2秒待ち） |
| pi-node単独・修正後 | 2成功、1失敗（NoOp 7の2秒待ち） |
| macOS workspace並行回帰 | 172成功、0失敗、1除外、47.85秒 |
| pi-node並行回帰 | 173成功、0失敗、6除外、3フィルター除外、50.56秒 |
| pi-node実XDP split recovery | 成功、17.20秒。Drop、期限によるPass復帰、XDP解除・隔離ping成功 |
| macOS/Linux Clippy、format/diff検査 | 成功 |

修正後失敗の `prepare-after-2.log` では、NoOp 7、leader d、term=1、wait=7,875〜9,876ms。フォロワーa/bのPrepare票保存は8,200〜9,443ms（1,243ms）、cは8,219〜9,444ms（1,225ms）だった。leaderのprepare証明＋自分のCommit票保存は9,481〜9,503ms。期限時点ではa/b/cのCommit票保存が9,631/9,623/9,612msから未完了、leaderのCommit票は自分のみだった。Prepare票は遅れても集約できたが、後続の必須保存まで2秒に収まらなかった。同一View内の経路であり、今回統合したView単独保存はここには存在しない。

修正前のNoOp 7失敗では3フォロワーが既に上のtermへ移行していた。修正前のもう1回はNoOp 3で状態処理中を伴う失敗だった。原因・段階が異なる失敗をひとまとめにして改善効果を計算しない。

**判定：新View時の重複保存削減は採用。単独試験の安定合格は未達。** 最小修正と安全性回帰、pi-node比較は完了したが、小規模サイト試験導入のマイルストーンは未完了である。既存の長い保存停止を解消したとはしない。必要な1回の保存を削除したり、同期完了前に投票したりする変更は含めていない。

今回の製品変更は `two_phase.rs` のView受理と投票の保存境界のみ。追加回帰は `two_phase_tests.rs` の5件。WAL、state_worker、daemon、daemon_tests、QUIC、policy_clock、consensus_engine、progress、electionの9ファイルは作業前hashと一致する。B3通常選挙失敗とTLS更新後3秒待ちは引き続き別件・未検証。commit/pushは行っていない。
