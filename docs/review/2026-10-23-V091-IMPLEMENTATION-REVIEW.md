# v0.9.1 実装・復旧・仕様突合レビュー

対象: `7e9473c1dec8a9f5667057295e708627ec2185e1` / 2026-10-23。

前回の [設計レビュー](../FEASIBILITY_REVIEW_AND_V1_DESIGN.md) は `c8d3f7f` の記録である。本書はその後の修正を含む現在の実装を検証する。旧文書の行番号・再現結果を現在のコードへそのまま適用しない。

**判定: 小規模サイトの自動遮断パイロットへ投入するには、まだ不足がある。** BFTだけでなく、期限切れ、緊急解除、永続化、再試行、実デーモンへの接続に欠陥が残る。製品コードの修正は今回行っていない。

最終ゴールは小規模サイトで安全に実践投入すること。現在のマイルストーンは投入条件と未達部分を根拠付きで確定すること。今回のタスクは通常の実装・仕様突合・回帰確認と、BFT・復旧の再レビューである。サイト構成・BFT必須性・許容停止時間は未決定であり、本書から自動的に信頼モデル変更や本番適用へ進まない。

## 1. 検証結果と境界

| 検証 | 結果 | 証拠の意味 |
| --- | --- | --- |
| macOSの元ソース: `cargo test --workspace --locked --offline --no-run` | 成功 | テストを含むコンパイル |
| 元ソース: `cargo fmt --all -- --check` | 成功 | 書式 |
| 元ソース: `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 成功 | 静的検査 |
| 元ソースの既存テスト15件 | 成功 | 下記に明示した状態機械・回帰ケース |
| 隔離コピーの追加再現8件 / macOS arm64 | 全件で欠陥を再現 | 合格を期待する製品テストではなく、現状の欠陥をassertする再現コード |
| 隔離コピーの追加再現8件 / Linux x86_64・pi-node | 全件で欠陥を再現 | 同じ再現の移植性。実NIC・実カーネルの期限切れを試した証拠ではない |

既存15件は `proof_1,3,4,5,8,10,11,12,13` と `regression_r1`、`regression_r2` の2件、`regression_r4_apply`、`regression_r9`、`regression_r10`。既存スイート全件成功とは報告しない。既存PKI・鍵・WAL・chaosテストにはファイル削除が含まれるため、削除の個別承認を要求する作業ルールに従い、今回それらを実行対象から外した。検証条件や製品コードの安全条件は変更していない。

再現は [追加テスト](2026-10-23-v091-reproduction.rs) を隔離コピーの `veil-node/src/usecase/consensus_engine.rs` 末尾に追加して行った。製品の通常テストへは組み込んでいない。WAL復元テストは現在の `WalPayload` / `WalEnvelope` と同じ形式を、新しい0600ファイルへ排他的に保存するfixtureを用いる。`save_wal()` 自体の削除・rename・電源断耐性はこのfixtureでは検証していない。

実wire試験は生成したテスト用CAでloopback QUICを使用した。他の再現は実際の署名検証・状態遷移を呼ぶが、ネットワーク配送をテストから制御する。実行中の既存サービスや実NIC設定は変更していない。

## 2. 投入を阻む指摘

### F1 / P1: 拒否したQC付きRPCがログを変更し、別の確定値への署名を可能にする

根拠: `consensus_engine.rs:826–836,847–884,728–742`。

ロック解除時のQC検査は署名・term・indexだけで、変更対象の候補チェーンとの対応を検査していない。候補ログへ書き換えた後にQCとローカルログの整合性を確認するため、そこで失敗してもログ変更が残る。後続の空AppendEntriesは、残った別ブランチに対するQC署名を返せる。

再現 `rejected_unrelated_qc_mutates_log_and_enables_conflicting_commit`:

1. A/B/C/D、定足数3。悪意あるノードはAだけ。Aが遮断Xを配り、XのQCをBだけへ届ける。Bはindex1にXを確定、C/DはXを未確定のまま保持する。
2. term2でXを継承するindex2のNoOpについて、A/C/Dの有効QCを作る。C/Dにはまだcommitを通知しない。
3. AがC/Dへ、index1の許可Yと、手順2の別チェーンのQCを送る。RPCは `success=false` になるが、C/Dのログは既にYへ置換されている。
4. Aが通常の空AppendEntriesを送るとC/DがYへ署名する。A/C/DのYのQCをC/Dへ渡すと、BはDROP、C/DはPASSを同じindex1に確定する。

暗号の破壊、誠実ノードの鍵の入手、再起動は不要。各RPCは正規の署名検証入口を通す。現在のR2回帰が通っても、この経路は防げていない。

修正条件: 候補チェーン上でQCと安全な解除条件を検証し、不正提案の拒否でlog・lock・投票履歴を変えない。正当な高いtermへの移行等を許す場合も、その効果を仕様で区別する。変更適用と署名を分離し、検証途中の状態へ署名できなくする。ただし、この局所修正だけをBFT全体の安全性証明にしない。

### F2 / P1: 投票応答が永続化を経由せず、再起動後に同じtermへ再投票する

根拠: `quic_mtls.rs:291–307`、`main.rs:179–188,222–228`、`consensus_engine.rs:1167–1220`。

受信ハンドラは状態遷移・署名後にそのまま応答する。WALへの依存も、保存成功を待つ経路もない。通常デーモンの後続保存は、自分からAppendEntriesを送信して応答を受けた経路に限られ、保存失敗も捨てる。フォロワーの投票を永続化する仕組みにはなっていない。

再現 `wire_vote_is_returned_without_durable_vote_record`: 初期状態のWALを用意し、実loopback QUICでAへのterm1投票を取得する。WALは変わっていない。そのファイルから復元したBが、同じterm1でCへも投票する。プロセスをkillした試験ではなく、受信ハンドラと保存済み状態からの復元の組合せを確認した。

修正条件: 投票・lockの永続化成功を署名応答の前提にする。I/O失敗時は署名送信を止め、readinessを落とす。永続化前後の各境界でプロセスを強制終了する試験が必要。ファイルへの `sync_all()` とrenameがあるだけで、実運用経路のCrash-Recovery完了とは扱わない。親ディレクトリの同期、残存一時ファイル、同一node IDの二重起動も別途設計する。

### F3 / P1: TTLは手動の仮想時計だけで、実時間・カーネルの期限切れがない

根拠: `consensus_engine.rs:1006–1013,1070–1098`、`main.rs` 全体、`veil-common/src/lib.rs:8–13`、`veil_xdp_kern.c:39–43,119–127`。

`monotonic_now_ns` は0から始まり、`advance_monotonic_clock()` をテストが呼んだ場合だけ増える。通常デーモンには呼出しがない。カーネルへ渡す `PeerPolicy` に期限がなく、XDPにも時計検査がない。`proof_13` はシミュレータの時計を手動で進めている。

再現 `elapsed_time_does_not_advance_ttl`: TTL1秒のルール設定後、実際に1.1秒待ってもclock=0、期限切れ回収0、判定DROPのまま。

修正条件: 執行backendが自律的に期限を守ること。XDP採用なら実カーネルの時計・deadlineを実装し、ユーザー空間の停止中にも期限でPASSへ戻る試験を行う。ユーザー空間にtimerを足すだけでは仕様S4を満たさない。SIGSTOP中、再開後、再適用時、再起動後の各意味を定める。

### F4 / P1: 緊急バイパスがシミュレータにしか効かない

根拠: `consensus_engine.rs:1102–1104,1148–1152`、`ports.rs`、`veil_xdp_kern.c`。

`set_emergency_local_bypass()` はboolを変えるだけでbridgeを呼ばず、カーネル側にはバイパスflagのmapもない。

再現 `bypass_changes_simulator_but_leaves_bridge_blocked`: MockEbpfBridge経由で遮断を適用後、バイパスをonにする。状態機械の判定はPASSへ変わるが、bridgeが保持する執行状態はDROPのまま。この再現はmock境界の確認であり、実XDPへの非接続はソースでも確認した。

修正条件: 所有する執行backendへ明示した操作を行い、成功確認後にバイパス有効と報告する。再起動・reconcilerが勝手に解除しない状態管理も必要。CLI/管理APIと実通信の復旧試験まで含めて完了とする。

### F5 / P1: WAL復元が期限切れBANを復活させ、バイパスも失う

根拠: `consensus_engine.rs:23–31,1006–1013,1260–1277`。

WALには期限・時計・バイパスが含まれない。復元は全commitログを先頭から再適用し、相対TTLを新しい0起点の時計へ加算する。

再現 `expired_rule_and_bypass_are_lost_on_replay`: TTL60秒のBANを61秒経過で失効させ、バイパスonの状態を現行WAL形式で保存する。復元すると同じBANが新たな60秒期限で復活し、バイパスはoffになる。元の保存形式で状態を表現できない点を確認したもので、`save_wal()` の電源断試験ではない。

修正条件: 同じ決定の再送・再適用・再起動で期限を延ばさない。期限起点、決定ID/revision、同一boot内のdeadline、boot変更、時刻不確実性、解除・緊急操作の保持を決める。旧レビュー6.3の期限契約も参照する。復元時に過去の全BANを実mapへ順次再投入することで、解除済みBANが一時復活したり、過去の件数でmap容量を使い切ったりしないよう、復元する最終状態を先に計算する。

### F6 / P1: 一時的な適用失敗が、その後の正常heartbeatでも回復しない

根拠: `consensus_engine.rs:866–885,1017–1066`。

適用失敗で `last_applied` を止める修正は入った。しかしフォロワーは `target_commit > commit_index` の場合しか適用処理を呼ばない。既にcommitした値の再送・heartbeatは未適用部分を再試行しない。

再現 `heartbeat_does_not_retry_failed_follower_apply`: index1確定時のbridge書込みを失敗させる。bridgeを正常に戻して5回配送しても、書込み試行は最初の1回だけ、commit=1/applied=0のまま。追加のcommitがない状況を自力で修復できない。

修正条件: commit進行と独立した、上限付き・観測可能な適用再試行を用意する。部分適用したbatchを再試行してもTTLや副作用を繰り返さない。実bridge側でも、解除時 `map.remove()`、LSMのmap不存在・型不一致・`map.set()` 失敗を成功へ変換しない。未装着のLSMはunsupported/degradedとして扱う。

### F7 / P1: 件数だけのchunkingで64KiB上限を超え、追随不能になる

根拠: `consensus_engine.rs:354–388,212–216`、`quic_mtls.rs:278`。

R9修正は1024エントリ上限だけを見ている。現在は1コマンド128ルールなので、小さいNoOpの回帰では通信サイズ超過を検出できない。

再現 `entry_chunking_still_exceeds_wire_byte_limit`: 128ルール入りの64コマンドで、署名・wire envelope込み75,659bytesになり、65,536bytes上限を超える。受信状態機械も拒否し、フォロワーのログは空のまま。

修正条件: 最終的な署名・QC・wire envelopeまで含むbyte数と件数の両方で分割する。単一エントリが収まらない場合は提案受付で拒否する。最大長node ID、QC付きheartbeat、catch-upの境界も試験する。

### F8 / P1: 実デーモンに選挙・提案・回復の駆動と正しいメンバー対応がない

根拠: `main.rs:145–151,195–244`、`consensus_engine.rs:712–725,891–910`。

`--peer` から作るIDは `peer-0-127.0.0.1:...` だが、相手は通常 `node-beta` 等を名乗る。相手の鍵を登録しても、応答のメンバー検査と複製cursorのIDが一致しない。通常起動ではFollowerからの選挙開始、view timeout発火、提案受付を駆動するコードがなく、TTL timer・バイパス管理API・metrics HTTP公開もない。TUIや `MeshNode::campaign()` をテストから直接呼べることは、通常デーモンの運用経路が完成している証拠にならない。

修正条件: node ID・公開鍵・証明書identity・addressを対応づける静的設定を起動時に検証する。独立プロセスを通常CLIで起動し、実際の受付APIから提案して合意・適用・期限切れまで到達させる。Pingの成功をクラスタ合意の成功指標にしない。

### F9 / P1: TCでリーダーが交代しても、異なる未確定prefixを解消できない

根拠: `consensus_engine.rs:527–674,793–837,915–930`。

一台の悪意あるリーダーが誠実3台へ別々の未確定prefixを送った後、TCは次のleader IDを決めるが、安全なブランチ回復処理がない。高いQCができるまで競合の上書きを許さない規則と、競合したままではQCができない状態が衝突する。`highest_qc` を保持するだけでは回復しない。

再現 `divergent_uncommitted_prefixes_stall_honest_successor`: Aだけが不正提案を行い、その後停止する。B/C/D間の配送をすべて実行し、TCで誠実リーダー3名へ順に交代させて各8回複製しても、全員commit=0のまま。これは有限の具体的な回復失敗の証拠であり、全スケジュールに対する形式証明ではない。

修正条件: 安全性を維持するlock/解除規則、欠けたブロックの取得、最高証明の継承、投票可能条件を完全な方式として定義する。F1を防ぐため単にロックを強くしても、この進行不能は解決しない。既知方式の採用判断と反例を止める局所修正を区別する。

### F10 / P1: 共有CA生成が排他的でなく、初期化エラーを成功にする

根拠: `quic_mtls.rs:45–81`、`main.rs:76–105`。

「排他生成」というコメントに対し、実装は `exists` の後に通常writeし、鍵・証明書の保存失敗もchmod失敗も捨てる。同時初回起動では別のCAを生成・上書きし、各プロセスが異なるCAを信頼する、またはディスク上の鍵と証明書が不整合になる経路がある。既存回帰 `regression_r3` は同一プロセス内で2インスタンスを順次作る試験で、同時初期化や独立プロセスのクラスタ運用を検証していない。

修正条件: CAを管理側で生成し、ノードには自分のleaf秘密鍵と公開trust bundleを配る設計を優先する。共有ディレクトリ方式を開発用に残すなら、初期化の排他・不完全状態・鍵/証明書対応・所有者/mode・エラー伝播を検証する。全ノードへCA秘密鍵を配ると、1ノード侵害でTLS証明書を発行できる。この点はEd25519 QCの偽造が可能になることとは区別する。公開鍵ディレクトリを誰が書けるかも明示する。レースや他ユーザーの鍵読取を今回実行したとは主張しない。

## 3. 前回修正の評価と仕様の差

| 項目 | 今回確認した進展 | 残る境界 |
| --- | --- | --- |
| 累積hash、過去投票履歴 | 元のR1/R2回帰は成功 | F1の別QC・拒否後状態経由とF9の回復経路は未防御 |
| 適用失敗の可視化 | `last_applied` を止める元の回帰は成功 | F6の再試行と、実bridgeの失敗伝播が不足 |
| Linux名解決 | ソースは `crate::ebpf_bridge` へ修正済み | 実機テストの前提不足・attach失敗を `Ok(())` にする条件は残る |
| 鍵生成 | Unix作成時0600、create_new、既存mode/symlink拒否が入った | CA生成は別実装でF10が残る。親ディレクトリ・所有者・既存ファイル読取の競合も別途検証 |
| map回収・送信元port例外 | ソースには解除時remove、4096件、C版のport例外除去が入った | 今回その配布objectの実NIC挙動を再検証していない。容量回収時のエラー無視は残る |
| TTL/bypass/metrics | 状態機械の関数と既存テストがある | 実時間・実カーネル・通常CLI・HTTP endpointまで到達していない |
| chaos試験 | 順序を固定した状態機械・WAL復元の結合ケースがある | 実プロセスkill、`tc netem`、独立ホスト、fsync境界、鍵rotationの試験ではない |

現設計書のv0.4.0はPrepare/Commitの2段階QC、fuzzingを要求するが、現行のwire型にはその2段階がない。v0.5.0のLPM/CIDR/IPv6、50ms集約、Unix socket、v0.9.1のnetem・runbook・鍵rotationも、版番号と状態機械テスト数だけで完了にはできない。特にS4の「デーモン停止中にもカーネル内で失効」とREADMEの「TTL・緊急バイパス実装・検証済み」は検証層を明示する必要がある。

## 4. 次の実装担当へ渡す完了条件

今回の追加8件は欠陥をassertしている。修正時は、再現の前提と配送順を保ったうえで、期待値を「競合commit不可・期限延長不可・実執行回復」へ変更し、恒久回帰へ移す。テストを削除・skipしたり、安全条件を弱めたりして通過させない。

1. **保証を正す。** 現在の実装・シミュレーション・実機確認・設計上の予定を文書上で区別する。BFT安全性、通常daemonでのTTL/bypass、Crash-Recovery完了という現在の読み取り方を解消する。
2. **通常運用の最小経路を通す。** 一意のメンバー設定、エラー伝播、永続化前の署名禁止、適用再試行、wire byte上限、実backendの期限と解除を実装する。複数機能を同時に増やさず、1つの決定が受付→確定→適用→失効する証拠を取る。
3. **BFTの方式を確定する。** F1とF9を同時に防ぐ状態遷移、durable lock、view変更を文書化する。F1の局所修正を行っても、既知方式と異なる部分の安全性・進行性は保留する。
4. **別プロセス・実障害で受け入れる。** 同時初期化、投票後kill、ディスクI/O失敗、未確定分岐後のleader交代、遅れたpeer、daemon SIGSTOP中の期限切れ、再起動時のBAN再活性化防止、緊急バイパス保持を試す。状態機械のテストとは別の証跡にする。

小規模サイトでも、この失敗は利用者の通信を止める。先にサイト構成と必要な脅威モデルを選び、既存WAF/nftables等で目的を満たせるか比較する。1台の侵害耐性が必須なら、中央署名配信へ無断で保証を下げない。まずshadow運用、次に限定した手動適用の条件を満たし、検知品質と復旧が確認できてから自動化を判断する。

本レビューの終了条件は上記の根拠・再現・受入条件の提示であり、実装や本番投入の完了ではない。Geminiへの直接送信、commit、push、PR作成はしていない。Linuxへのソース転送と試験は、ユーザーが指定した `ssh pi-node` の範囲で行った。

## 5. 再実行と証跡

以下はtracked HEADの新しいコピーを作り、そこだけへ再現コードを加える。既存ファイルの削除は行わない。Mac/LinuxのUnix環境用で、fixtureは `/tmp` 相当へ保存して残す。build cacheの管理はCargo標準動作に従う。

```python
import io
import pathlib
import subprocess
import tarfile
import tempfile

root = pathlib.Path.cwd()
copy = pathlib.Path(tempfile.mkdtemp(prefix="veil-v091-review-"))
source = subprocess.check_output(["git", "archive", "7e9473c"])
with tarfile.open(fileobj=io.BytesIO(source)) as archive:
    archive.extractall(copy, filter="data")
test = root / "docs/review/2026-10-23-v091-reproduction.rs"
with (copy / "veil-node/src/usecase/consensus_engine.rs").open("a") as f:
    f.write("\n" + test.read_text())
print(copy)
subprocess.run([
    "cargo", "test", "--manifest-path", str(copy / "Cargo.toml"),
    "--locked", "--offline", "v091_review", "--", "--nocapture",
], check=True)
```

作業時の証跡ディレクトリ: `artifacts/review-v091-he6w9rjv/`（gitignored、ローカル保管）。`source-commit.txt`、`source.tar`、`baseline-selected.txt`、`macos-reproduction.txt`、`linux-reproduction.txt`、`remote-path.txt` を含む。今回追加した文書と再現コード以外のtracked fileは変更していない。

Linux条件: `Linux 7.0.0-38-generic x86_64`、既存NixのRust/Cargo 1.98.1、既存vendorによるoffline build。試験は新しい `/tmp/veil-v091-review-SaJVo8om` で実施し、root・実NICへのattach・既存checkoutの上書きは行っていない。Linux専用の既存テストも含めてコンパイルされたが、実行したのは追加8件だけである。実行コピーと生成fixtureは削除せず残している。

実行設定はセッション記録で `gpt-6-astra` / `high` を確認。担当は単独、今回のサブエージェント使用なし。前回の複数エージェント・Ultraの実行結果と混ぜて性能比較しない。
