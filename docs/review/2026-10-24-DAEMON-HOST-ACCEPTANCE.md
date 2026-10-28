# ビルド復旧と通常daemonの両ホスト比較

2026-10-24。[前回の独立ホスト比較](2026-10-24-INDEPENDENT-HOST-COMPARISON.md)で未実施だった通常daemon経路の追試。

## 結論と完了範囲

**ビルド経路の復旧と、両ホストの通常daemon比較は完了。現行方式を維持する。実サービスで遮断する試験導入はHOLD。**

pi-node・pi-masterそれぞれ1回、既存ハーネスの全10段階が成功した。TLS更新後の適用待ちは0.615秒／0.832秒で既存3秒以内。
認証付きcheckpoint、停止ノードへの状態転送、圧縮済みWALからの復旧、期限切れBANの非復活、復旧後の新しい合意・失効まで成立した。
両ホストとも記録された147待機区間に期限超過はなく、今回の試験プロセスの残存は0。

最終ゴールは安全に運用・復旧できる無償OSS。今回完了したのは通常daemon経路の成立確認であり、実サービスの実パケット遮断と独立した緊急復旧は未実施。
最終的な導入契約は[最小構成と導入ゲート](2026-10-24-PILOT-DECISION-AND-TARGET-DESIGN.md)を参照。

## 保存と安全条件

- 現行FileWalの完全snapshot置換、fileと親directoryの`sync_all`を維持。
- TTL、署名CLIの2秒backdate、時計誤差予算、各waitの制限、定足数、保存前の署名送出禁止を維持。
- 製品・テスト・ハーネスの編集なし。今回記録したソース・manifest・lockfile・BPF object等68ファイルの前後SHA-256が一致。
- pi-nodeのbuild元とローカルの製品ソースも一致。pi-master用バイナリは前回と同一ソースからの既存crossbuildをhash確認して使用。
- root操作、NTP・quota・mount・firewall・service設定変更なし。既存のbuild成果物、ログ、鍵、WAL、証拠directoryは削除していない。

## pi-nodeのビルド復旧

`/tmp`のlive mount設定は`tmpfs`、`usrquota`あり。homeは従来と同じ`/dev/sda5`のext4。
前回の`Disk quota exceeded`はこの環境と整合するが、ユーザーquotaの上限・使用量は取得していない。filesystem全体の空き容量だけではquota余裕を判定しない。

今回の変更はビルド実行環境だけ:

1. home配下に新規の0700作業directoryを作成。
2. `TMPDIR`をその中のcompiler専用directoryへ設定。
3. `CARGO_TARGET_DIR`を新しい出力directoryへ設定し、`CARGO_INCREMENTAL=0`で実行。
4. 既存Rustを明示指定し、`--locked --offline`と既存vendorを使う。

GCCは`TMPDIR`でコンパイル中の一時ファイルの場所を指定できる。[GCC公式資料](https://gcc.gnu.org/onlinedocs/gcc/Environment-Variables.html)
出力先と増分ビルドの設定はCargoの既存機能を使用する。[Cargo公式資料](https://doc.rust-lang.org/cargo/reference/config.html#buildincremental)
tmpfsのquotaはfilesystemの総容量とは別の制約である。[Linux kernel公式資料](https://docs.kernel.org/filesystems/tmpfs.html)

新規buildは59.995秒、exit 0。同じ専用領域でのbuild再実行もexit 0だった。
旧領域の重複symbolの詳細原因は未確定。新しい出力領域・増分無効・一時領域を同時に変更しているため、どの条件が重複symbolを解消したかまでは分離していない。
**共有`/tmp`自体のquota問題を修復したという意味ではない。通常daemonを作成できる専用のビルド経路を復旧した。**

今回確認したpi-nodeで同じ方法を使うコマンド例。毎回新しい作業領域を作り、既存成果物を削除しない:

```sh
task_build="$(mktemp -d "$HOME/vdf-build-XXXXXXXX")"
mkdir -m 700 "$task_build/compiler-tmp"
cd /home/maakie/veil-censor-47iX3Rbs
TMPDIR="$task_build/compiler-tmp" \
CARGO_TARGET_DIR="$task_build/target" \
CARGO_INCREMENTAL=0 \
RUSTC=/nix/store/53lvgpxsb3b9ghkgb4v0jra6zzlnyw8n-rustc-1.98.1/bin/rustc \
/nix/store/gmgfpzc0w2pgby0lmq9r88m39albiyfw-cargo-1.98.1/bin/cargo \
  build -p veil-node --locked --offline \
  --config 'source.crates-io.replace-with="vendored-sources"' \
  --config 'source.vendored-sources.directory="/home/maakie/veil-mesh-consensus/target/vendor"'
```

このNix store pathとsource/vendor pathは今回のホストで確認したもの。他環境の汎用インストール手順ではない。
実記録ではrustc 1.98.1、同じNix toolchainに含まれるcargo表示は1.98.0。Rustコンパイラ版とCargo版を同一視しない。

## 比較条件と試験結果

両ホストで同じ`daemon-flow-check.py --checkpoints`を、通常の独立4プロセス・実QUIC/mTLS・実Linux時計・永続WALで実行。
新しいfixtureの運用者鍵・node鍵・CAは0700 directory内で生成した。daemon用TLS bundleにCA秘密鍵がないことをハーネスが確認した。
これは同じUID・ホスト内のfixtureであり、運用者と各validatorの権限分離や4物理ホストの故障独立性を証明しない。

| 項目 | pi-node | pi-master |
| --- | --- | --- |
| 実行環境 | x86_64、SATA SSD、ext4 | Cortex-A76、microSD、ext4 |
| 初期起動・自動選挙・NoOp | 成功 | 成功 |
| 承認付きBAN・適用・失効 | 成功 | 成功 |
| follower経由の要求・合意・失効 | 成功 | 成功 |
| 通常leader停止・再選挙・同鍵WAL復帰 | 成功 | 成功 |
| 少数派の保留・期限後catch-up | 成功 | 成功 |
| TLS証明書/鍵更新・旧証明書の失効配布 | 成功 | 成功 |
| 更新後BANの適用待ち、上限3秒 | **0.615202秒** | **0.831751秒** |
| 個別解除・旧承認再送拒否・改ざん拒否 | 成功 | 成功 |
| checkpoint境界 | 32、64 | 32、64 |
| 遅延nodeの状態転送・圧縮WAL復帰・その後の合意/失効 | 成功 | 成功 |
| 既存ハーネスの全体 | exit 0、10段階成功 | exit 0、10段階成功 |
| 記録された待機の期限超過 | 147件中0 | 147件中0 |
| ハーネス全体の実行時間 | 156.346秒 | 127.193秒 |

全体時間はBANのTTL待ち等を含む。保存時間、単一要求の応答時間、処理能力を意味しない。
CPU・kernel・媒体・linker等が異なり、媒体だけを交換したA/Bではない。各1回の成功から長期安定性や失敗率を推定しない。

TLS更新で合意署名鍵が変わらず、commit済みWALが保持されたことを確認。
checkpointではreceipt hash、commit index、last_applied、保存/適用エラーなし、期待するpolicy mirrorを組み合わせて検証した。
単に`compacted`と表示されたことだけで合格にはしていない。
最終snapshotでは全nodeがcheckpoint 64、期限後のpolicy mirrorはPass、保存/適用エラーはNoneだった。
最後の時計不確かさ標本はpi-node 7.5 ms、pi-master 136.127 ms。全時間の時計誤差上限の実測ではない。

### 3秒の起点と補助判定

3秒waitはTLS更新・選挙の後、`ban()`が承認生成、submit、ログ登録確認、receipt取得を終えた位置から始まる。
署名から3秒、要求受付から3秒、TLS更新作業全体が3秒という意味ではない。
TTL=4と2秒backdateは維持した。4秒全部が署名後に残るという解釈もしない。

既存`wait_for`はpredicate内のCLI待ちを含むため、predicateが遅いと制限超過後に成功を返す余地がある。
ハーネスを変更せず、結果の全`completed_waits`について`elapsed_secs <= limit_secs`を独立に確認した。今回は超過0。
将来の最小修正候補はdeadlineをpredicate完了後にも検査すること。今回その修正や時間制限の変更は行っていない。

### 証拠の境界

- `--xdp`なし。`policy_mirror=Drop`は論理適用であり、パケットDROPではない。
- 既存reportは`unrelated_ports_pass=True`等をXDPなしでも書くため、その項目は実パケットの証拠に数えない。未実施を明示する改善は別の修正候補。
- 通常leader停止の試験はB3相当。fresh応答を返す検閲leaderの交代試験は[前回の単独/並行回帰](2026-10-24-INDEPENDENT-HOST-COMPARISON.md)であり、今回と混同しない。
- kernel TTL、実通信、保存停止中の緊急解除、電源断、4物理ホストの障害試験、形式的BFT安全証明は今回未実施。

## バイナリと記録の確認

| 対象 | SHA-256 |
| --- | --- |
| pi-node通常daemon | `a7720bc94600118cffd15ee77663d6cc60c8b73a20c96282b7f514f557f00efc` |
| pi-master通常daemon | `d546de38981e1018c6614d7cf7e12c07c84701d711a224cbb758177c02518566` |

ARMバイナリは前回確認したCortex-A76専用crossbuild。A53固有linkerオプションの扱いを含む制約は前回報告のままで、汎用ARM releaseの認定ではない。

build再実行の補助JSONには、初回と異なるhashなのに`same_binary=true`という不整合があった。その肯定値を証拠に採用せず、元記録を保全した。
実ファイルを`sha256sum`とPythonの独立した計算で再確認したところ、双方とも初回build・flow起動前の上記hashと一致した。
補助記録の不整合原因は未確定。`binary-recheck.json`に再確認値と除外理由を記録した。

## 導入判断と次の境界

今回の結果は、前回の「通常daemon経路が未確認」というゲートを解消する。
新しい保存形式、同期省略、TTL延長、時間制限延長を選ぶ必要はない。

実際に遮断する限定導入の前に残る主要ゲート:

1. **対象サービスの確定と実通信の確認。** 送信元が識別可能な限定scopeで、正常通信、BAN、期限、個別解除、scope外の非影響を実パケットで確認する。
2. **独立管理経路による復旧。** WAL保存停止を含む状況で、所有プロセス終了・FD解放・XDP link解除・通信回復を運用担当者が確認できること。local bypassは状態ロックを共有するため代替にならない。
3. **配置の故障境界。** 各ホスト内4nodeの成功を4独立障害領域と数えず、媒体・管理権限・配布権限の共通故障と鍵/状態復元の運用を定める。

このため「通常daemon経路の比較完了」と「実サイト導入HOLD」を両立させる。未合意のroot操作・実ネットワークへの配備・公開を開始しない。

## 証拠の保存場所

- ローカル: `artifacts/daemon-host-pwiw2id9/`
- pi-node: `/home/maakie/vdf-u5r578q4/`
- pi-master: `/home/take/vdf-wr2u8r9p/`

主要索引は`build-native-result.json`、`build-native.log`、`flow-executions.json`、両ホストの`flow-result.json`、`acceptance-summary.json`、`post-state.json`、`source-before.json`/`source-after.json`、`binary-recheck.json`。
両ホストで今回の実行ファイルに属する残存プロセス0、各4 WAL、0700の証拠directoryを確認した。試験終了後も旧/新のデータを保全している。
