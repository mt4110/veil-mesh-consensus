# GCP承認Aの実施：VM作成503・新設network撤去完了

> 後続: 本人承認による[us-central1-b一回比較も503で停止・撤去完了](2026-10-26-GCP-A-ZONE-B-COMPARISON.md)。以下の「未承認」の候補は、この報告時点の判断を保全したもの。

2026-10-26 JST。Sol／High、分担なし。[準備・認証待ち](2026-10-26-GCP-A-PREPARATION.md)の後、本人のCLI再認証完了を受けて承認Aを実施した。

**二台のVM準備・隔離受入れは未達。保護対象VMの作成APIが二回ともHTTP503で失敗し、client VMは要求していない。新設SSH rule・subnet・VPCは所有IDを照合して撤去し、既存resourceの9区分が開始前と一致した。Bは未実施。**

最終ゴールは公開サイトで安全にBAN・解除・失効・復旧できること。今回の停止箇所は、その前のGCP VM作成であり、製品・XDP・BANの動作試験の失敗ではない。GCP全般が非対応とも判定しない。

## 課金前に確認したこと

| 対象 | 結果 |
| --- | --- |
| CLI認証／project | `veil-x-tests`の読取り成功、project ACTIVE、既存請求先有効。共有default project変更なし |
| 衝突・既存資源 | VM／disk／予約IPは0、既定network一件、subnet42、firewall4、route43。作成予定名は全て不存在 |
| OS／machine | `ubuntu-2604-resolute-amd64-v20261021`、image ID `5898740419867771658`、READY、X86_64、通常版Ubuntu license。e2-smallは共有CPU・memory2048MiB |
| quota | us-central1: CPU200／使用0、disk4096GiB／使用0、使用中外部IPv4 8／使用0、instance24／使用0。全region CPU32／使用0 |
| OS Login | 有効なrequireOsLogin policyにenforced設定なし、project metadataにもenable-osloginなし。既存policyをFALSEに変える操作なし |
| 管理source | Macのdefault routeは有線en0。en0を指定した外向きHTTPS照会で、承認AのM/32と一致。携帯回線・Pi・hp・router操作なし |
| 請求 | accountはopen、通貨JPY。日本向け10% JCTを含む計画はUS$0.242相当。無料枠なしのUSD計算に基づく見込みで、JPYの実請求・丸め・月次換算は確定値ではない |

税・通貨の扱いは[Googleの税説明](https://docs.cloud.google.com/billing/docs/resources/vat-overview?hl=ja)と[通貨換算の説明](https://docs.cloud.google.com/billing/docs/resources/currency)を確認した。US$1は今回の増分に対する運用上限で、全accountやモデル利用料を含む強制課金停止機構ではない。

## 実行時間と作成・削除したもの

- T0: **14:46:50 JST**。最初の新設resource要求前に記録した。
- 予備のVM自動削除予定: 18:31:50 JST、全体上限: 18:46:50 JST。要求に同じ絶対期限を含めたが、**VMは成立せず、この予定が稼働中VMへ登録されたわけではない。**
- 最終の撤去・保全照合: **14:58:32 JST**。約11分42秒の実行窓で終了。これは製品性能の測定値ではない。

| 新設対象 | 所有ID | 最終状態 |
| --- | --- | --- |
| VPC `vlb-gcp-20261026` | `3108558260831213476` | 同一ID照合後に削除、不存在を確認 |
| subnet `vlb-gcp-20261026-us-central1` | `2588425119851566962` | 同上 |
| SSH rule `vlb-gcp-20261026-ssh` | `1294933733693151010` | M/32→今回のtarget tagのTCP22のみで作成し、同一ID照合後に削除 |
| VM／boot disk／外部IPv4 | 作成されたIDなし | 二回目失敗後と最終照合の両方で0件 |

新しい試験専用SSH鍵はMacのprivate artifact内で生成した。公開鍵だけをVM作成要求へ含め、秘密鍵を出力・送信していない。VM不成立のため、稼働するVMへのSSH認証には使用していない。localの鍵・要求・失敗・証拠は削除せず保全する。

## VM作成の失敗と一回の限定再試行

`us-central1-a`の`veil-ban-target-test`へ、承認どおりのimage、e2-small、新規pd-standard20GiB、service account／scopesなし、Secure Boot／vTPM等、専用network、絶対時刻DELETE、boot disk auto-deleteを明示した。

初回のCLIは`Could not fetch resource: Internal error. Please try again or contact Google Support`で終了1。VM・disk一覧は空、直後の操作一覧には今回のnetwork等のinsert三件だけがあり、VM insertは見えなかった。固定imageとpd-standardの読取りは成功した。

CLI logから**HTTP method・Compute path・statusだけ**を抽出し、VM挿入POSTへの503と確認した。auth headerやtoken、元のlog全体を報告へ複製していない。

残存VM／diskの不存在と、失敗が503であることを新しい判断材料として、一度だけ遅延して同じ要求・同じ期限を再試行した。原要求と失敗receiptを上書きせず、`target-retry1`へ別保存した。二回目もPOSTが503、CLI終了1。二回目の後もVM・diskは空だったため、ここで打ち切り、client VMは要求しなかった。

| 試行 | zone読取り | VM挿入POST | 結果 |
| --- | --- | --- | --- |
| 初回 | HTTP200 | HTTP503 | VM／diskを確認できず |
| 限定再試行一回 | HTTP200 | HTTP503 | 同上。以後の再送なし |

503は[Compute APIのerror catalog](https://docs.cloud.google.com/compute/docs/reference/rest/v1/errors)にも複数の原因で存在する。本件のmessageだけから在庫不足、Google内部障害、特定requestとの相互作用を断定しない。403や具体的なquota超過は表示されなかったが、基盤の内部原因は未特定。別zone／別image／条件緩和の試験は実施していない。

## 撤去と保全の確認

VM作成失敗を「何も残らないはず」と扱わず、VMとdiskの一覧を再取得した。存在しないことを確認してから、作成成功receiptのあるSSH rule→subnet→VPCだけをID照合して削除した。

削除helperは、対象IDの変化、boot diskの付替え、未知の依存、失敗要求の後に残る所有不明VM／diskでは停止する。曖昧なVMを削除しない拒否経路も、外部操作なしで確認した。force、全firewall flush、前方一致での一括削除はしていない。

その後、別の読取りで同じ投影項目を再取得し、順序を正規化してbefore／afterを比較した。

| 区分 | 最終件数 | 開始前との比較 |
| --- | --- | --- |
| VM | 0 | 一致 |
| disk | 0 | 一致 |
| network | 1 | IDと取得設定が一致 |
| subnet | 42 | 同上 |
| firewall | 4 | 同上 |
| address | 0 | 一致 |
| route | 43 | IDと取得設定が一致 |
| snapshot | 0 | 一致 |
| resource policy | 0 | 一致 |

これは取得したresource／項目の比較で、account全体の全サービス・全fileの無影響証明ではない。Compute APIと既定生成物は以前の状態で保全。今回の停止後に課金対象となる新設VM・disk・外部IPv4は観測されないが、実請求額の確定照合は未実施とする。

## 今回実施していないもの

VM上のpackage準備・SSH疎通・隔離実XDP・実NICへの装着・BAN／解除／期限／restart、18443 rule、DNS保存、公開証明書発行は全て未実施。先行hpでの成功や今回のオフラインguard確認を、これらの合格へ転記しない。

## 次の判断候補（未承認・未実施）

同じIowa地域の`us-central1-b`に限定した一回の比較なら、region料金・image・machine・M/32・B保留を変えず、zone依存かを調べる材料になる。同zoneのe2-smallのmachine定義は読取り確認できたが、在庫や作成成功は保証しない。

承認Aの固定zoneを変更するため、本人確認なしには実施しない。実施するとしても**今回のT0とUS$1を引き継ぎ、T0や四時間をリセットしない。** 新規のprivate実行記録を使い、同名resourceの不存在を再照合する。target作成一回が成功した場合だけclientを一回作り、失敗したら撤去して終了。元の準備上限15:46:50 JSTと予備削除18:31:50／全体上限18:46:50も維持し、残時間が足りなければ作らない。これはB承認ではない。

## 証拠

private artifact: `artifacts/gcp-a-live-lq6nhlrx/`。

- [最終結果と9区分の照合](../../artifacts/gcp-a-live-lq6nhlrx/final-summary.json)、[開始・終了と所有ID](../../artifacts/gcp-a-live-lq6nhlrx/session.json)。
- [初回要求](../../artifacts/gcp-a-live-lq6nhlrx/target-request.json)・[初回失敗](../../artifacts/gcp-a-live-lq6nhlrx/target-receipt.json)、[限定再試行](../../artifacts/gcp-a-live-lq6nhlrx/target-retry1-request.json)・[再試行失敗](../../artifacts/gcp-a-live-lq6nhlrx/target-retry1-receipt.json)、[HTTPの限定抽出](../../artifacts/gcp-a-live-lq6nhlrx/creation-http-summary-final.json)。
- [最終VM](../../artifacts/gcp-a-live-lq6nhlrx/final-instances.json)・[最終disk](../../artifacts/gcp-a-live-lq6nhlrx/final-disks.json)、各区分のbefore／final JSON、所有ID照合と削除のcleanup JSONL。
- [作成phase helper](../../artifacts/gcp-a-live-lq6nhlrx/phase.py)・[撤去helper](../../artifacts/gcp-a-live-lq6nhlrx/cleanup.py)。削除は本人承認Aの新設対象に限り実施済みで、別の既存resourceを削除する手順として使わない。

秘密鍵・credential・tokenは証拠リンクへ含めない。原失敗を成功へ書き換えず、再試行と復旧を分けて保全した。[試験履歴](../local-ban/TEST_HISTORY.md)へ戻る。
