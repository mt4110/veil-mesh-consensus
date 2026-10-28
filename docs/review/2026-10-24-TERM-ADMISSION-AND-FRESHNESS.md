# term移行の定足数確認とheartbeat再送への対処

最終ゴールは小規模サイトで安全に試験導入できる状態。今回の対象は、正規メンバー1台の高いtermで合意が停止する問題と、保存した署名付きheartbeatで選挙期限を延ばせる問題。通常daemon、署名RPC入口、WAL復旧、シミュレータの選挙経路を更新した。BFT全体の進行性が完成したという意味ではない。

## 修正前の反例

変更前の作業ツリーを保存して、独立したコピーで再現した。

- 4台構成でterm 1の選挙を済ませ、正規メンバーが `MAX_SAFE_TERM - 1` のRequestVoteを送ると、受信側のtermは `9223372036854775806` になる。以後の通常選挙は上限で拒否される。RequestVoteResponseとAppendEntriesResponseも、単独の高いtermを降格根拠にしていた。
- 同一の署名付きAppendEntriesを再送すると、内容が進んでいなくても `election_activity` が増え、daemonの選挙期限が延びる。外側の署名やmTLSだけでは、署名した時点と受信時点の近さを確認できない。

## termを進める条件

起動直後のterm 1は共通の開始点として扱う。それ以降、他ノードの大きな数値だけで `current_term`、投票先、roleを変更しない。ローカルの再選挙も、単独でtermを繰り返し増やさない。

通常daemonは選挙期限が切れると `PreVote` を送る。これは事前確認であり、永続的な候補者投票ではない。受信側は次の条件で現在termの次のtermを支持できる。

- 要求が自分の `current_term + 1` で、有効な上限内にある。
- 自分がLeaderではなく、直近1.8秒以内にfreshなleader応答を確認していない。
- 候補者が示す最終ログのterm/indexが、自分より遅れていない。

支持署名は `VEIL_TERM_SUPPORT_V1`、署名context、対象termに束縛する。contextには既存のcluster ID・config epoch・固定構成digestが含まれる。同じvoterの応答は1件として数える。4台なら3台の支持で `TermCertificate` が成立し、その後、既存のRequestVoteを実行する。事前支持は非排他的で、candidateへの投票権やログのQC署名を与えるものではない。

証拠付きのRequestVote・AppendEntries・それらの応答は、新しい `TermCertified` RPCで送る。受信側は、別々の登録メンバーによる定足数署名、証拠のterm、内外のsender/term一致を検証する。単独の代理配送で追随できるのは、中に定足数の署名があるためである。内側はサイズ制限付きで一度だけ復号し、証拠の再帰的な入れ子を認めない。wireの64KiB制限には証拠のサイズも含める。

実際の候補者投票は、これまでどおり `voted_for` のWAL保存、同termでの二重投票拒否、ログの新しさ検査を経る。ログ採用・QC検証・prefixのロック・適用前永続化・有限TTL検査を省略しない。term証拠はLeaderの選出証明でも、コマンドのコミット証明でもない。

現在termの証拠と、次termのために集めている証拠は別々に保持する。旧heartbeatが現在termの証拠を再提示しても、次の選挙準備を消せない。旧TimeoutCertificate経路にも上限検査を適用し、同じtermを再適用して保存済み候補者投票を上書きしない。

## 選挙期限を延ばす条件

AppendEntriesの受理自体では選挙期限を延ばさない。Followerは認識しているleaderへ `LivenessProbe` を送り、OS乱数で生成した256bit nonceへの署名付き `LivenessAck` を求める。

- Probeの有効期間は送信側の単調時計で800ms。term・期待する相手・nonce・現在認識しているleaderが一致する必要がある。
- Ackは一度だけ消費する。次のProbe、term移行、再起動で古いnonceは使えない。
- 応答側は同じtermのLeaderである場合だけAckを作る。
- 新しい候補者投票を永続化したときは、選挙の往復時間を与えるため一度だけ期限を延ばす。同一投票要求の再送では延ばさない。

したがって、内容の変わらない正常なleaderもfreshな応答で期限を延長できる一方、保存済みheartbeatやAckだけでは延長できない。時計合わせや署名付きの送信時刻には依存しない。既存の選挙待ち時間1.8〜3.6秒、200ms駆動周期、800msのRPCラウンド上限と組み合わせる。これらは現在の運用値であり、任意のWAN遅延での進行保証ではない。

## WAL v5と復旧

成立した現在termの証拠をWAL v5へ追加した。次termの未成立な事前確認、challenge nonce、受信時刻は保存しない。更新されたstateの永続化に失敗した場合は、既存の停止ラッチに従い、そのまま投票を続けない。

WAL v4も読み込める。元のterm、投票先、ログ、QC、entry署名履歴、BAN期限、bypass状態を維持し、読み込みだけでは元ファイルを書き換えない。次の正常なcheckpointからv5で保存する。v1/v2/v3、破損、異なる署名context、禁止された旧BANの拒否は維持する。v5のterm証拠も署名とterm整合性を検査する。

再起動したノードはFollowerから再開する。保存された証拠を遅れた候補者へ提示でき、各ノードのtermがずれていても、単独の未証明な高いtermを信じる必要はない。v4由来で証拠がないノードについては、現在termの支持観測を定足数で集める復旧経路を使う。旧版が既に異常に高いtermを保存した場合や、証拠のない旧WAL同士が複雑に食い違う場合の自動修復は保証しない。termを下げる、WALを消す、`--init-state`を再実行する復旧は行わない。

署名context v3、構成digest、TLS SAN、コマンドとQCの署名形式は維持した。新しいRPCタグを使うので、旧binaryとの混在をサポートする変更ではない。全validatorを停止し、最新WALと鍵を保全して全台更新する。v5を書き込んだ後の旧binaryへの単純なダウングレードは不可。古いsnapshotへの巻き戻しを代替手順にしない。

## 検証範囲

- 署名が正しい高termの要求・応答・TimeoutVoteを拒否し、WAL・投票先・role・選挙期限が変わらない。高いPreVoteもtermを進めない。
- 同一voterの重複支持、偽装したvoter集合、異なるcontextの支持、内外sender不一致、入れ子証拠を拒否する。
- 少数派で何度pre-voteしてもtermが膨張しない。定足数成立後の通常投票・コミットは進む。
- 同一heartbeat、同一投票、使用済みAck、前回ProbeのAck、期限後Ackを再送しても期限は延びない。freshなAckは成功する。
- 旧heartbeatを連続処理しても次termの準備を保持する。WAL v4からの投票保持、v5証拠の再起動・遅延配送、checksumを合わせた不正証拠の拒否を確認する。
- node別TLSを使った実QUICで高term要求を繰り返し送る間に通常選挙・BANコミットが進む。選出済みleaderのserver/driverを止め、そのleaderのTLS資格と署名を使って保存済みheartbeatを再送し続けても、残る3台が新しいtermで選挙・NoOpコミットを完了する。再送が最初から拒否されていただけでないことも成功応答数で確認する。

実QUICの攻撃試験は同一ホスト内の状態機械と実daemon driverを使う。別プロセス4台と実XDPの試験は通常の運用経路を確認するものであり、攻撃試験と混同しない。以下はこの実装に対する検証結果である。

## 残る試験導入の条件

今回の変更は、旧署名の再送と単独の未証明termによる停止への対処である。freshなProbeへ応答しながらコマンドを検閲するByzantine leader、競合する未コミットprefix、敵対的な選挙の繰返し、資源枯渇を含む完全なBFT view-changeの進行性証明ではない。DoSによる通信・CPU・ディスクの飽和も別途扱う必要がある。

小規模サイトでの自動遮断の試験導入は、配置と執行点、source IPの見え方、時計の不確かさ、監視、権限、復旧担当と実サイトの受入条件を決めてから判断する。単一Linuxホストの隔離試験だけで、本番投入可とは判定しない。

## 最終検証結果と証拠

| 検証 | 結果 |
| --- | --- |
| 修正前の隔離コピー | 高termによる上限到達、同一heartbeatによるtimer延長の2件を再現 |
| macOS workspace回帰 | 101成功、0失敗、1 ignored |
| pi-node / Linux x86_64回帰 | 101成功、0失敗、5 ignored、3 filtered |
| 両環境Clippy | 全target、`-D warnings`で成功 |
| Linuxの追加実QUIC攻撃試験 | 高term要求147回、保存済みheartbeatの成功応答133回を観測。残る3台がterm 1→2へ再選出し、index 3までコミット |
| Linux専用の実XDP試験 | 有限BAN・解除、禁止BAN拒否、署名期限catch-up、SIGSTOP失効、bypass、WAL復旧が成功 |
| WAL起動black-box | 欠損拒否、初期構築の一回性、鍵lock、同一WAL復旧など9項目成功 |
| 通常daemon 4プロセス | BAN適用/失効、再選出、WAL復帰、少数派catch-up、TLS更新/失効を含む6段階成功 |
| 独立した終了後確認 | 今回のdaemon残留0、XDP残留なし、隔離ネットワークのping PASS |
| ソース照合・静的検査 | 実行ソース等53ファイル一致。fmt、diff、Markdown 22ファイル成功 |

ignored/filteredを成功数に含めない。Linuxの環境依存/TUI試験3件は除外した。必要なLinux専用試験は別途明示実行した。TUIは非対話シミュレータの再選挙経路を検証したもので、対話UI全操作の実機受入れではない。攻撃通信の回数は今回の観測値であり、負荷耐性の保証値ではない。

途中の失敗も保存した。旧heartbeatが次termの準備を上書きする競合は追加した実通信試験で発見し、状態を分離して決定的な回帰試験も追加した。macOSの初回並列実行で既存WAL lock試験が一度失敗したが、後続の全体試験と該当試験の単独再実行は成功した。この単発lock失敗の原因は確定していない。Linuxの初回ビルドは `/tmp` のuser quotaで失敗し、ホーム領域にソース・build出力・コンパイラ一時ファイルを置いて再実行した。試験条件や安全機構を無効化していない。

証拠は `artifacts/term-freshness-bln_zx5x/`。変更前のsource archive、2件の再現ログ、今回だけの `this-task.patch`、両OSの検証ログ、`daemon-flow-result.json`、`wal-startup-result.json`、`source-hash-comparison.json`、終了後の確認ログを保存した。検証用Linuxコピーは `/home/maakie/veil-term-YnkJLeLz`。容量制限で止まった `/tmp/veil-term-GzGI2h5R` も削除していない。

実packet検査は既存の `veth-victim` / `ns-veil-rogue` のみ。物理NIC、host時計/NTP、既存firewall、本番設定は変更していない。fixtureと旧作業内容を保全し、秘密鍵を回収用証拠へ含めず、commit/push・公開は行っていない。
