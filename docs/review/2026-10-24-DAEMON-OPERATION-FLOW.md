# 通常daemonの自動選挙・BAN受付・合意・期限失効

対象: 固定membership・署名protocol v3・WAL v4の通常daemon。2026-10-24（Asia/Tokyo）。

最終ゴールは小規模サイトで安全に試験導入できる状態にすること。今回のマイルストーンは、通常daemonの起動から管理者によるBAN、合意・適用、期限失効までの経路を成立させること。以下の実装と隔離Linux受入試験を完了した。本番投入やBFT全体の安全性・進行性の完成を意味しない。

## 実装した経路

1. 既存の固定構成・署名鍵・WAL検証、Linux時計設定、任意のXDP backend、mTLS endpoint、所有者限定control socketを起動する。
2. 後続修正で、選挙タイマー更新はfreshなchallenge応答または新しい永続投票だけに限定した。期限切れ時は定足数のpre-vote後にRequestVoteへ進む。[term移行・freshnessの契約](2026-10-24-TERM-ADMISSION-AND-FRESHNESS.md)を参照。
3. 定足数を得たリーダーは、そのtermのNoOpを一度提案する。既存prefixを現在termのQCでコミット可能にし、その後もAppendEntriesとQCを送る。
4. 所有者がローカルCLIで有限TTLのIPv4 BANを提案する。リーダーは既存の保護IP・時計・署名期限・WAL検証を通してログへ記録し、受付票を返す。
5. 既存のQC検証で合意し、各ノードが自身のbackendへ適用する。時計の不確実性と配送時間を差し引く。期限切れ履歴は合意履歴に残しても新しいBANとして発動しない。
6. 実XDPの期限判定はカーネルで実施する。daemonは200ms間隔で適用再試行とミラーの期限整理を行うが、パケットの期限失効はそのループに依存しない。

選挙待ちはOS乱数による1.8〜3.6秒、駆動tickは200ms。peerごとの通信を同時に進め、1回のRPC全体を800msで打ち切る。これはタイマー設定であり、選出時間や障害復旧時間のSLOではない。fsync、scheduler、通信状態で遅れる。選挙・複製中のネットワーク待ちでは状態lockを保持しない。

通常daemonから疎通Pingの待ち合わせを除いた。Ping成功を合意参加や進行の代用にせず、署名付きの選挙と複製を直接駆動する。駆動処理またはcontrol serverが停止した場合、daemonはエラー終了する。Ctrl-Cでは所有endpointとbackground taskを終了する。control socketファイルは暗黙に削除しない。

## 管理者CLIと成功の意味

起動時の `--member`、`--cluster-id`、`--config-epoch`、既存鍵、WALの契約は[固定membershipの仕様](2026-10-24-FIXED-MEMBERSHIP-DIGEST.md)のとおり。署名protocol/WAL形式は今回変更していない。各ノードで明示的な `--control-socket` を指定すると運用しやすい。親directoryはdaemon所有者の0700、socketは0600、接続UIDも検査する。再起動時は新しいsocket pathを用意する。

```sh
# 対象daemonの所有者として実行する。例のIPは実際の試験対象へ置き換える。
veil-node --control-socket /run/veil-control/current/node.sock --status-local
veil-node --control-socket /run/veil-control/current/node.sock \
  --ban-ip 192.0.2.55 --ttl-secs 30
```

BANの受付例:

```text
ACCEPTED index=2 term=1 hash=<64桁hex> not_after_unix_ns=<署名対象の共通期限>
Acceptance is durable; quorum commit and local apply must be checked ...
```

**このコマンドのexit 0はWALへの受付成功であり、合意成功や遮断成功ではない。** フォロワーは `not leader` と既知のleader IDを返して拒否し、自動転送しない。管理者はそのリーダーのローカル管理経路で操作する。TTLは1〜86400秒が必須で、0や省略による無期限BANはこの入口では許可しない。TTLはログ投入時点から進み、合意待ち時間を含む。

受付票のindexを指定して確認する:

```sh
veil-node --control-socket /run/veil-control/current/node.sock \
  --status-local --entry-index 2 --status-ip 192.0.2.55
```

| 項目 | 意味 |
| --- | --- |
| `role`, `leader`, `term`, `quorum` | ローカルで把握する選挙状態。leader表示だけで定足数到達を保証しない |
| `entry_index`, `entry_hash` | 受付票との対応。**hashも照合する**。未確定entryは再選出で置換され得る |
| `entry_phase=absent` | そのindexのローカルログがない |
| `entry_phase=pending` | ログを保持するが未コミット |
| `entry_phase=committed` | QCに基づきコミット済みだがローカル適用が未完了 |
| `entry_phase=applied` | その履歴までローカル状態への適用が完了。現在もBANが有効とは限らない |
| `backend=none` | 実執行backendがない。このノードのappliedはカーネル遮断の証拠にならない |
| `backend=configured` | backendを設定済み。apply error、実counter、対象パケットの結果も確認する |
| `policy_mirror` | 指定IPv4についてのローカルミラー判定。独立したパケット試験の代用ではない |
| `kernel_pass`, `kernel_drop` | backendが取得できる場合だけ返す実counter |
| `persistence_error`, `apply_error`, `lease_clock_error` | 永続化・適用・直近のlease時計取得のエラー。常時の外部時計監視ではない |

各ノードのstatusはローカル観測である。クラスタ全台の適用完了を主張する場合は全台の受付票hash・適用状態を確認する。期限を過ぎた履歴の `applied` と `policy_mirror=Pass` は正常な組み合わせである。

応答喪失、timeout、WALエラー後の受付成否は不確実になり得る。自動再送によるTTL更新はしない。現在のログと受付票を確認する。request IDによる一般的な冪等再送APIは今回実装していない。

既存の `--bypass-local on/off --control-socket ...` は維持した。管理要求は最大8接続、各2秒の制限を持ち、完全な固定長入力を受けるまで状態lockを取らない。遅い1接続で他の管理要求を塞がない。ただしdaemonがSIGSTOP、停止、全体的な資源枯渇の状態ではsocketによるbypassは使えず、カーネルTTLと別の管理経路が必要になる。

Linux以外の通常daemonはシミュレーション時計へfallbackせず、新規TTL提案を拒否する。TUIや明示的なテストfixtureのシミュレーション動作とは区別する。

## 接続時に修正した問題

- 同じtermのリーダー通知でCandidateからFollowerへ移る際に、`voted_for` を消していた。これを保持し、そのtermで別候補へ再投票しないようにした。WAL再読込後も保持する回帰テストを追加した。
- 選挙termの上限で無効な署名メッセージを出さずに拒否する。
- 共有CAが未初期化のdirectoryへ複数daemonが同時起動すると、別々のCAを生成して上書きできた。安定したOS file lockで初期化を直列化し、0600/create-new/fsyncで秘密鍵を保存する。片方だけ存在するCA pairは上書きせずエラーとする。これは共有CA方式を本番用PKIへ置き換える変更ではない。

## 検証と証拠

| 検証 | 結果 |
| --- | --- |
| macOS `cargo test --workspace --locked --offline` | 79成功、0失敗、1 ignored |
| pi-node通常回帰 | 78成功、0失敗、4 ignored、3 filtered |
| 両環境のall-target Clippy / ローカルfmt・diff・Markdown確認 | 成功 |
| 実QUIC自動駆動 | 4 memberのうち先頭1台が不達でも残り3台で選出・BAN合意・適用 |
| 管理入口 | follower/保護IP/不正TTLを拒否、遅い入力と並行したbypassが成功 |
| 従来のLinux専用試験 | 署名付き期限catch-up、SIGSTOP TTL、期限切れWAL復旧、バイパス、未登録鍵拒否が成功 |
| 通常daemon 4プロセスのCLI受入試験 | 以下の5段階が成功 |

Linux通常回帰のfilteredは既存のTUI/環境依存試験 `proof_6_tui_arena`、`proof_7_live_linux`、`regression_r6_r7_linux`。ignoredの専用試験をすべて合格扱いせず、実行したものを別に記録した。macOSだけの追加1件は、通常daemonの時計が新規TTLを拒否するテスト。

通常daemon受入試験は `scripts/daemon-flow-check.py`。テスト内部の `start_election` や `propose` 呼出し、WAL fixtureの注入は使わない。通常binaryで鍵を明示初期化し、全4daemonを同時起動し、CLI経由で操作する。

1. 起動・自動選挙・NoOp合意と状態確認。
2. フォロワーへのBANと保護IPのBAN拒否。ログindexが増えないことを確認。
3. 有限BANの全台同一hashでの合意・適用、実packet DROP。XDP所有daemonをSIGSTOPしたまま署名期限を過ぎるとPASSへ戻る。再開後の全台ミラーもPASS。
4. SIGSTOP復帰後の**現在のリーダーを再確認してから**Ctrl-Cで停止。残り3/4台の別leaderがより新しいtermでBANを合意し、旧leaderを同じ鍵・WAL、別socket pathで起動するとcatch-upする。
5. フォロワー2台を停止した2/4の少数側ではBANが `pending`、commit index不変、PASSを維持。期限経過後に定足数を復元すると履歴を合意・適用しても古いBANは発動しない。

pi-node上の4プロセスは同一Linuxホストのloopbackで通信する。実XDPは既存の `veth-victim` 1本にだけ設定した。他の3ノードは実カーネル執行を持たない。複数物理ホスト、複数実NICでの独立した執行試験ではない。

再実行例（既存の隔離fixtureを事前に確認する）:

```sh
cargo build --workspace --locked --offline
sudo python3 scripts/daemon-flow-check.py \
  --binary target/debug/veil-node \
  --output-dir /tmp/veil-daemon-flow-new-run \
  --xdp assets/veil_xdp_kern.o
```

output directoryは未存在であること。スクリプトは既存XDPを検出したら開始を拒否し、物理NIC・host時計・NTP・既存firewallを変更しない。自身の子processのみを終了し、証拠directory、veth、namespaceを削除しない。終了後はXDP残留なし・ping成功を検査する。スクリプトの `--xdp` を省略すると実packet/カーネルの試験は行わず、同等の執行証拠にはならない。

今回の保存先:

- ローカル: `artifacts/daemon-flow-smyhwp37/`
- pi-nodeの隔離source: `/tmp/veil-daemon-FGv0jmqv`
- 通常daemon最終受入れ: `/tmp/veil-daemon-FGv0jmqv/flow-final/result.json`
- 従来TTL/復旧試験: `/tmp/veil-live-lease-856796-50931344492075`
- source archive、今回差分、実行ログ、WAL、結果JSONを保持。回収archiveに秘密鍵を含めない。

最初の追加テストはfixtureの参照型の誤りでコンパイルに失敗し、型を修正して再実行した。受入スクリプトはSIGSTOP復帰による再選挙を考慮し、停止直前のleader/termと停止後のterm増加を明示検査するよう強化して最終実行した。判定条件や既存の安全検証を緩めていない。

## 残る投入条件

通常daemonの基本経路は接続できた。次の課題は別に残る。

- BFTのview-change、異なる未コミットprefix、遅延・再送・敵対的な選挙での安全性と進行性。今回の乱数タイマーと通常のleader停止試験はその証明にならない。署名済みleaderメッセージの再送による選挙抑制まで解消したとは主張しない。
- 共有CA秘密鍵からノード別TLS identity・配布・失効運用への移行。複数ホストでの設定/鍵配置と受入試験。
- 設定/epoch移行、ログ・map容量、過負荷、制御プロセスの権限分離と監視。今回の少数台試験は容量・性能保証ではない。
- 導入先の選定と管理経路の保護。IPv4 exact IP、設定した保護IP、時計品質、実backendを前提とし、IPv6/CIDRや全ネットワークの防御を意味しない。

本番自動遮断は引き続き保留。WAL削除・identity再生成で移行や復旧の制約を回避しない。

## 後続のTLS運用変更

[ノード別TLS証明書・鍵の運用](2026-10-24-NODE-TLS-OPERATIONS.md)により、通常daemonは `--tls-dir` を必須とし、共有CA秘密鍵の生成・読込を廃止した。起動引数と受入スクリプトは後続仕様を優先する。本記録の共有CA方式・本番PKI未実装という説明は記録時点の状態である。複数ホストへの本番配布、自動更新・即時失効、site受入れは引き続き未完了。

## 後続のWAL初期構築変更

通常起動は正常な既存WALを必須とし、初回の空state作成は `--init-key` が発行する一度限りの許可と `--init-state` へ分離した。新規クラスタの起動例にはこの操作が追加で必要。既存鍵で欠損WALを再作成する操作ではない。[起動契約・互換性・復旧の境界](2026-10-24-WAL-BOOTSTRAP-RECOVERY.md)を優先する。

## 後続のterm・freshness変更

高term単独署名と保存済みheartbeatによる停止への対処、およびWAL v5への証拠保存は[追加実装契約](2026-10-24-TERM-ADMISSION-AND-FRESHNESS.md)を優先する。本書の試験数は当時の記録であり、最新の追加検証は同契約に記録する。
