# nftables候補：権限境界・停止復旧の設計とオフライン検証

2026-10-27。本人のGOは①設計と②独立試作・オフライン検証まで。最終ゴールは限定公開サイトへの安全な試験導入、今回のゴールは権限・状態・停止復旧の境界を具体化して、実機へ進める条件を判定すること。

**独立したRustモデルと再現runnerを作り、27件のRust検証、Clippy、format、実適用コマンド拒否を確認した。現行XDP・workspace・lockfile・objectの保全も確認。実nftables、実SO_PEERCRED、特権分離のOS検証は未実施。実機適用はBLOCKEDを維持する。**

後続の本人選択で現行の絶対期限を維持し、BAN適用を保留した。[owner限定確認の準備記録](2026-10-27-NFT-OWNER-PROBE-PREPARATION.md)に、空tableだけのscript・隔離案・15件のoffline検証を別記する。以下の当初記録と失敗証拠は保全し、後続の準備を実機成功へ読み替えない。

## 変えたもの

- [独立crate](../../experiments/nft-boundary/): 標準ライブラリのみ、unsafe禁止、root不要。既存workspaceへ追加していない。
- [固定frameと状態機械](../../experiments/nft-boundary/src/lib.rs): 信頼境界のモデル、入力拒否、固定scope、単一mutation、結果不明、所有socket終了と不存在確認。
- [fault injection](../../experiments/nft-boundary/tests/boundary.rs): メモリのReferenceKernelを用いる境界・障害検証。実packetやkernelではない。
- [再現runner](../../scripts/nft-boundary-offline-check.py): private evidence生成、offline／locked build、各終了コード・標準出力・stderr、既存製品七ファイルのhash前後比較。
- [設計書](../local-ban/NFT_BOUNDARY_DESIGN.md): 最小権限、owner table、停止・再起動、期限の反例、実機・公開の未検証を明記。

実Netlinkや実nftを呼ぶadapter、権限付与、systemd unit、実機へ適用するscriptは作っていない。今回のモデルは、既存の256件・有限TTL・保護IP・revision・一件ずつの変更という概念を参考にした別実装。既存backendの適用実績をnft版へ移したとは扱わない。

## 検証結果

環境はmacOS、rustc 1.98.1、cargo 1.98.1。追加依存の取得・sudo・SSH・cloudはない。再現コマンドは次のとおり。

```sh
python3 scripts/nft-boundary-offline-check.py
```

[runner結果](../../artifacts/nft-boundary-check-uba0ewri/result.json)、[Rustテスト出力](../../artifacts/nft-boundary-check-uba0ewri/tests.stdout)、[build・テスト起動情報](../../artifacts/nft-boundary-check-uba0ewri/tests.stderr)、[Clippy](../../artifacts/nft-boundary-check-uba0ewri/clippy.stderr)、[demo](../../artifacts/nft-boundary-check-uba0ewri/demo.stdout)を保全。

| 検証 | 結果と範囲 |
| --- | --- |
| Rust unit | 1/1。revision枯渇時にwrapせず、状態を変更しない |
| Rust integration | 26/26。下記の故障・境界。実kernel試験ではない |
| format／Clippy | 成功、warningをerror扱い |
| `offline-demo` | 結果不明→Quarantined→Closing→不存在確認後Stopped。apply_supported=false |
| `--apply`、引数なし | どちらもexit 2。暗黙の実行・実適用を受け付けない |
| 既存製品の保全 | Cargo.toml／Cargo.lock／veil-nodeのmanifest、local_banのcore／kernel／runtime、XDP objectの七ファイルで前後hash一致 |

integrationでは、不正長・magic・version・opcode・予約領域、認証前の入力拒否、別UID／namespace／socket、command文字列の追記、保護IP／TTL0／期限overflowを確認した。previewが状態を変えないこと、古いinstance／revision、256件と257件目、満杯時に既存entryを退避しないこと、一件だけのin-flightとrate制限も確認した。

障害では「kernelへ送る前」と「kernelが反映した後」のACK消失を区別した。どちらも成功へ変換せず、再送・後続mutationを拒否する。旧BANを短縮する途中の不明結果は古い長い期限も保持し、解除の不明結果も解除済みにしない。初期化／prepare／commit／ACKの四境界で終了イベントを注入し、tableが残る間は新しい起動を拒否、不存在確認後だけ新instance・BAN0を許可した。

掃除の16件制限はuserspaceのモデルだけ。kernel側のexpired entryが残る状況と別に扱い、これで実nftのGC時間・メモリ・再挿入成功を保証しない。

自己レビューでは、初期モデルの完了応答がinstanceを保持しておらず、socket ID・revision・内容が再利用されると旧世代の応答を受理する反例を再現した。[修正前の失敗](../../artifacts/nft-boundary-offline-yh4npr9d/old-generation-counterexample.log)を保全し、transactionと完了照合にもinstanceをbindingした。最終回帰ではこのcaseを含めunit1件＋integration26件が成功した。既存製品には変更していない。

## 一次資料から得た二つの判断

### 停止復旧: owner tableを修正採用

通常のpersistent tableをflushする追加watchdogを作る前に、既存のowner機能を使う。**専用の長寿命Netlink socketでowner tableを作り、persistを付けない**候補に固定した。kernelは所有socketの終了を検知してtableを回収する実装を持つ。[Netfilter公式man](https://www.netfilter.org/projects/nftables/manpage.html)・[Linux v6.8の実装](https://github.com/torvalds/linux/blob/v6.8/net/netfilter/nf_tables_api.c#L10681)。

子processが所有FDを保持した場合や、brokerだけが残った場合は「管理画面が終了したから回収された」とは言えない。FDの非継承・管理処理との終了連動・最小権限・実際のSIGKILLと通信復帰は、次のOS検証に残す。今回のモデルはそこを証明していない。

### 期限: 素朴な相対TTLへの差替えは不採用

現行XDPのBOOTTIME絶対期限を、nftの相対TTLへ同等とみなして置き換えることは避ける。確認したv6.8のkernel sourceは、element初期化時のtick値と相対期間で期限を作る。[element初期化](https://github.com/torvalds/linux/blob/v6.8/net/netfilter/nf_tables_api.c#L5956)・[期限比較](https://github.com/torvalds/linux/blob/v6.8/include/net/netfilter/nf_tables.h#L769)。

モデルの反例は、時刻0のTTL30秒をkernelが1秒で初期化すると相対期限が31秒になる、というもの。**入力を置いた反例計算で、1秒の実測値ではない。** suspend中のclock進行が異なる場合の反例もモデル化した。対象hostのkernelのsuspend・丸め・順序はまだ測っていない。

`DeploymentEvidence`は未確認のOS条件を列挙するモデルで、booleanをtrueにすること自体を証拠とはしない。実適用adapterが存在せず、CLIも常にapply_supported=falseを出すため、モデルの成功から実適用へ進む経路はない。

## 採否と残り

| 対象 | 判定 |
| --- | --- |
| 型で制限した管理操作・状態量・失敗時の隔離 | モデル段階の採用候補 |
| owner付き・persistなしの所有socket lifecycle | 一次資料に基づく採用候補、対象kernelで未検証 |
| CAP_NET_ADMINの実分離・namespace confinement | 設計のみ、未検証。table所有機能だけで権限を閉じ込めたとは言わない |
| 現行と同じ絶対期限 | 未達。単純相対TTLの移植は不採用 |
| 実機適用・公開試験導入 | BLOCKED／HOLD。モデル検証を③や製品完成と同一視しない |

**設計とオフラインモデルの検証は実施済み。②の実機試験scriptは期限契約の判断待ちで、①②全体は未完了。追加のクラウド作成へ進む段階ではない。** 現行の期限を保つ方法が必要か、kernel適用時からの相対期間等へ契約を変えるかを明示して判断する。後者は未承認で、今回自動変更しない。所有機能だけの限定実機確認を先行させる場合も、対象・影響・停止・復旧・既存設定保全の具体案を別途示す。

GCP／AWSの前回停止・撤去記録は保全し、今回再起動・作成・課金・状態照会をしていない。hp／Piへ接続せず、ホストの防火壁・network namespace・systemd・account・network設定を変更していない。commit／push／公開なし。

[設計契約](../local-ban/NFT_BOUNDARY_DESIGN.md)・[試験履歴](../local-ban/TEST_HISTORY.md)・[実装の読み方](../../experiments/nft-boundary/README.md)。
