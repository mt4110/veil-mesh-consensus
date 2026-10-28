# 独立ホストでの現行WAL比較と見極め結果

2026-10-24。追加2段階・実作業1〜2日相当を上限とする環境比較。製品コードの改修は行っていない。

後続の明示依頼でビルドを復旧し、通常daemonの両ホスト比較を完了した。[後続結果](2026-10-24-DAEMON-HOST-ACCEPTANCE.md)を参照。
以下は当初の停止時点の記録として保全する。現在の導入判断は[採否・最小構成](2026-10-24-PILOT-DECISION-AND-TARGET-DESIGN.md)を参照。

## 判定

**現行方式を維持して見極める方針は支持する。試験導入はHOLD。新しい保存方式への移行は採用しない。**

- 第1段階の独立ホスト確保と、同一ソースによる試験バイナリの準備は完了した。
- 第2段階の保存比較、検閲リーダー交代、並行回帰は完了した。通常daemonの比較はビルド準備で止まり、未完了。
- 保存遅延には環境差がある。しかし今回の比較は媒体だけを交換した実験ではなく、CPU・kernel・媒体・ホスト負荷等が同時に異なる。原因をSSDそのものへ限定しない。
- 計測した61ファイルの前後SHA-256は一致。WAL形式、file/親directoryの`sync_all`、TTL、既存期限、定足数、投票前永続化を変更していない。
- 本結果は無償OSSの限定導入候補を評価するもの。実サイトの保護、一般的BFT安全性、多ホスト障害耐性、製品完成の認定ではない。

到達構成と導入ゲートは[採否・最小構成の設計](2026-10-24-PILOT-DECISION-AND-TARGET-DESIGN.md)を参照。

## 環境と比較条件

| 項目 | pi-node | pi-master |
| --- | --- | --- |
| 物理ホスト | maakie-lab | ra-master-5、別ホスト |
| CPU | x86_64、i7-7700、公開4 CPU | aarch64、Cortex-A76、4 CPU |
| kernel | 7.0.0-38-generic | 7.0.0-1020-raspi |
| 永続媒体 | EAGET SSD1TB、SATA、`/dev/sda5` | microSD、`/dev/mmcblk0p2` |
| filesystem | ext4、rw,relatime | ext4 |
| RAM / swap | 約31 GiB / 約8 GiB | 約8 GiB / なし |
| 試験中のWAL場所 | home配下、SSD | home配下、microSD |

pi-nodeには別の使用可能な物理媒体を確認できなかった。別directory・partitionを独立媒体とは扱っていない。
両ホストの`/tmp`はtmpfsであり、WAL比較には使用していない。短い試験directoryについても証拠directoryと同じdeviceであることを確認した。
既存Colima環境も確認したが、本比較の実行先には採用していない。

各ホストで4論理ノードを実行した。4台の独立物理ホストで構成する1クラスタの試験ではない。
単独試験は実QUIC/mTLSと現行FileWalを使用するが、執行は試験用backendである。実パケットのDROPを測っていない。

### ソースとビルド

同一の現在ソース、Cargo.lock、Rust 1.98.1（48a229cea）、既存vendorを使用。製品依存を追加・更新していない。
pi-node側ソースmanifestとローカルmanifestの一致、選択試験inventoryの一致を確認した。

pi-masterにはRust/ccがなく、Macの今回の作業directory内で公式Rustのtarget標準ライブラリとZig 0.15.2を使いcrossbuildした。
target tripleをZig表記へ変換し、検査済みCortex-A76向けにA53固有erratum対策linkerオプションを除いた。
**このバイナリは対象A76ホスト専用の検証用であり、Cortex-A53や汎用ARM配布用ではない。**
ホストへのtoolchainインストール、system設定変更は行っていない。初期のtoolchain不一致・linker失敗の記録も保全した。
対象固有オプションの根拠は[GCC AArch64公式資料](https://gcc.gnu.org/onlinedocs/gcc/AArch64-Options.html)。

| バイナリ | SHA-256 |
| --- | --- |
| pi-node試験 | `471d04fbcddc438a86ffa4197c398d0c1fdc01b52f2a02f55639f0ed3aa65a22` |
| pi-master試験 | `0097aaba91ce139a2e3d48f9294e4dc253525cb9881ce1582f12c9ccd4d8c844` |
| pi-master通常daemon、buildのみ | `d546de38981e1018c6614d7cf7e12c07c84701d711a224cbb758177c02518566` |

pi-node試験バイナリ名にappendが含まれるが、試験のbackendは明示的に`VEIL_TEST_WAL=replace`で固定した。
実行ログにも`backend=replace`を確認した。追記試作の結果を混在させていない。

## 保存比較

既存のcontrolled storage試験を使用。16 KiB payload、writer当たり64保存、1 MiB cap、1または4 writer。
初期ファイル作成は保存時間の計測外。保存時間には一時ファイル書込み、file同期、rename、親directory同期が含まれる。
2巡目はホストの順番を逆転。両ホストで同一の条件を使い、全8実行は成功した。

単位ms。値は各有限標本の中央値 / p95 / 最大であり、運用SLOや最悪時間の保証ではない。

| writer数・巡 | pi-node | pi-master |
| --- | --- | --- |
| 1・1 | 20.036 / 31.992 / 56.623 | 10.693 / 15.206 / 27.297 |
| 1・2 | 19.235 / 54.943 / 154.453 | 9.735 / 12.189 / 14.918 |
| 4・1 | 43.264 / 60.243 / 78.379 | 25.527 / 31.694 / 47.652 |
| 4・2 | 45.104 / 65.482 / 819.603 | 23.476 / 26.505 / 43.685 |

この条件ではpi-masterの保存遅延が小さかった。「microSDはSSDより速い」「全負荷で1秒停止が解消した」とは結論しない。
合意試験中のpi-masterでは別途303 msの保存も観測した。保存単体の最大48 ms弱を、全体の上限として使わない。

## 合意と並行回帰

| 試験 | pi-node | pi-master |
| --- | --- | --- |
| 検閲リーダー交代、単独 | 3/3成功 | 3/3成功 |
| 並行回帰、2 thread、訂正後の対応する2巡 | 各183成功、7 ignored | 各183成功、7 ignored |
| 並行回帰の試験全体時間 | 52.73 / 50.49秒 | 166.15 / 167.64秒 |
| 単独3巡内のsampled WAL最大 | 846 ms | 303 ms |
| 訂正後の並行2巡内のsampled WAL最大 | 815 ms | 45 ms |

単独試験は`automatic_driver_replaces_fresh_censoring_leader_and_preserves_request_deadline`。
freshな応答と無関係なcommitを返す検閲leaderから交代し、誠実な3nodeで元の期限のまま適用・失効することを確認した。
sampled WAL最大は出力された標本の最大であり、全保存の厳密な最大ではない。

並行回帰は同じ190試験を選択し、183成功・7 ignored。TUI・root XDP実験を除外した。
x86_64だけに存在するroot XDP試験は選択外なので、filtered数のみpi-node=3、pi-master=2。
ARMの全体時間には192通りのprepare配送・leader組合せ等の検証が含まれ、合意期限を延ばした値ではない。
外側の240秒watchdogは発火していない。既存のNoOp 2秒や検閲試験内の期限は同じ。

### 実験側の失敗と訂正

最初のpi-node並行回帰は181成功・2失敗。原因は今回設定した長いTMPDIRによるUnix socketの`path must be shorter than SUN_LEN`。
失敗したのはlocal controlとowner socketの2試験で、検閲試験は成功していた。これは実験設定の不備として扱う。

既定の停止条件でいったん止め、TMPDIRだけを同じhome filesystemの短い固有名へ訂正した。
製品コード・テスト期待値・安全条件・時間制限は変えず、両ホストの同条件pairを再実行した。
初回pairは対応比較の合格件数から除外し、失敗ログ・停止記録・訂正理由を保存した。後続成功で初回失敗を消していない。

## 通常daemonの未完了と停止理由

通常CLIによる起動、実時計の承認、TLS更新後の3秒BAN適用、checkpoint復旧を、既存`daemon-flow-check.py --checkpoints`で比較する準備を行った。
実XDPを伴う`--xdp`はこの非特権比較の対象外としていた。

pi-nodeの通常daemonビルドは以下で止まった。

1. 既存target領域では`symbol '' is already defined`、exit 101。原因は未確定。
2. 既存領域を保全し、新しいtarget・増分ビルド無効で一度だけ準備確認したところ、ringのCコンパイルが`/tmp/...: Disk quota exceeded`、exit 101。

後から確認した`/tmp`には約3.29 GBの空きがあり、「ディスク全体が満杯」とは判定しない。quotaの詳細原因は今回追っていない。
この/tmpはビルド用一時ファイルの場所で、成功したWAL比較の保存場所ではない。
ソース変更、削除、quota変更、system設定変更で救済を重ねず停止した。ARM通常daemonはbuild成功済みだが、両ホストとも通常flowは開始していない。
したがって**今回の通常daemon比較・TLS更新後3秒待ち・実時計条件・通常選挙B3の成否は未確認**である。
これは製品runtimeのタイムアウト再現とも、方式の不成立証明とも扱わない。

## 残る導入ゲート

1. 再現可能な通常daemonのbuildと、同じ既存条件の通常flowを完了する。これは今回の準備問題を解消した後の別の入口であり、小改修の自動継続ではない。
2. 対象の1サービスで正常通信、BAN、期限、個別解除、scope外への非影響を実パケットで確認する。
3. 保存停止中でも運用担当者が外部管理経路を使い、所有プロセス終了・XDP link解除・通信回復を確認できること。local bypassは同じ状態ロックを待つため、独立復旧の代用にしない。
4. 4論理nodeの成功を4独立障害領域へ読み替えず、実際の配置・鍵・媒体・管理権限の共通故障を明記する。

今回の結果から新保存方式・非同期投票・時間制限延長を提案する必要はない。
一方、未実施の通常運用・復旧・実サービス試験が残るため、限定試験導入HOLDを維持する。
準備不成立で止めるという合意を適用し、追加の修正ループへは入っていない。

## 証拠と保全

ローカル証拠は`artifacts/independent-host-boica20v/`。主な索引:

- `pi-node-inventory.json`、`pi-master-inventory.json`、`remote-paths.json`
- `source-before.json`、`source-after.json`、`source-unchanged.json`（61ファイル一致）
- `pi-node-source-match.json`、`selected-test-inventory.json`、`cross-build-boundary.json`
- `comparison-plan.json`、`results.json`、`stop.json`、`corrected-plan.json`、`corrected-results.json`
- `storage-summary.json`、`regression-summary-final.json`、各実行のlog/result
- `build-daemon-native-result.json`、`build-daemon-native-fresh-result.json`、対応するbuildログ
- `pi-node-post-state.json`、`pi-master-post-state.json`

両ホストで今回の試験バイナリの残存プロセスは0と確認。通常daemonは未起動。
証拠・失敗したビルド・試験directoryは保全した。ファイル削除、root操作、NTP・firewall・サービス設定変更、公開、commit/pushは行っていない。
