# GCP承認A：us-central1-b一回比較・503で停止・撤去完了

> 後続: [作成要求・既存監査ログの読取り分析](2026-10-26-GCP-503-REQUEST-ANALYSIS.md)で、code14・七権限granted・boot／image／期限の整合を確認。追加作成なし、根本原因は未確定。

2026-10-26 JST。Sol／High、分担なし。[us-central1-aの失敗・撤去](2026-10-26-GCP-A-VM-CREATE-503.md)の後、本人がzoneだけの変更と一回の比較を承認した。

**us-central1-bのtarget VM作成もHTTP503で失敗した。再送・client作成・他zoneへの変更はせず、新設network等を撤去した。VM二台の準備・GCP実XDPは未達、撤去と既存resourceの照合は完了。公開Bは保留のまま。**

最終ゴールは公開サイトで安全に手動BAN・解除・失効・復旧できること。今回のマイルストーンは、その前段のVM準備。今回の完了条件は、元の費用・時間・安全条件のまま一度比較し、失敗時に新設物を撤去して証拠と履歴を保全することだった。

## 変更したものと維持したもの

- zoneだけを`us-central1-a`から`us-central1-b`へ変更。新しいrun ID・所有receiptを使い、前回の要求・失敗・鍵・証拠を上書きしない。
- 同じproject `veil-x-tests`、e2-small、Ubuntu `ubuntu-2604-resolute-amd64-v20261021`、新規pd-standard20GiB、IPv4-only、専用VPC／subnetを維持。
- Macの有線en0から管理sourceを再照合。SSHは同じM/32から今回のtarget tagのTCP22だけ。service account／API scopesなし、Secure Boot／vTPM等とboot disk auto-deleteを維持。
- 公開18443・DNS・証明書発行は行わない。Pi・hp・router・Macネットワーク設定や既存serviceへ操作しない。携帯テザリングを使わない。
- 同時BAN256件・再起動でBAN消失・有限TTL等の製品契約には変更なし。製品コードも変更していない。

原T0は**14:46:50 JST**、準備上限15:46:50、予備削除18:31:50、全体上限18:46:50、費用上限US$1のまま。helperで前回sessionとの同値を確認し、T0を再設定する処理を除いた。VM要求にも元の削除予定時刻を含めたが、**VMは成立せず、自動削除予定が実VMに登録されたわけではない。** US$100を今回の予算へ読み替えない。

## 一回の比較結果

作成前の一覧はVM0・disk0・network1・subnet42・firewall4・address0・route43・snapshot0・resource policy0で、対象名に衝突なし。zone bのe2-small定義の読取りと管理source一致を確認した。

専用network等を再作成し、**15:15:03 JST**にtarget VM作成を一度要求、**15:15:06**にCLI終了1となった。エラーは前回同様、`Internal error. Please try again or contact Google Support`。CLI logからmethod・Compute path・statusだけを限定抽出した。

| 比較 | zone読取り | target VM挿入POST | その後 |
| --- | --- | --- | --- |
| 前回us-central1-a | 200 | 503、限定再試行も503 | client未要求、新設物撤去済み |
| 今回us-central1-b | 200 | **503、一回のみ** | client未要求、再送なし、新設物撤去済み |

失敗後の独立したVM／disk一覧は0件。今回の時間帯のoperation一覧にはnetwork／subnet／firewallのinsert三件だけがあり、VM insertは観測されなかった。

**zone変更だけでは解消しないことは確認できた。内部原因は未特定。** 在庫不足、project固有の問題、Google内部障害、request条件との相互作用のどれかを、このmessageだけで断定しない。二zoneの失敗は全zone・全imageの非対応を意味しない。逆に、HTTP503を製品・XDP・BANの試験失敗とも扱わない。VMが成立する前に停止している。

## 撤去・保全

| 今回の新設物 | 所有ID | 最終状態 |
| --- | --- | --- |
| `vlb-gcp-20261026` VPC | `4731938272449876303` | 同一ID照合後に削除、不存在確認 |
| `vlb-gcp-20261026-us-central1` subnet | `5935732453497065772` | 同上 |
| `vlb-gcp-20261026-ssh` firewall | `2128946451918836031` | 同上 |

前回と同名だが、IDは今回の作成receiptのものだけを使った。失敗したVM要求についてはVM／diskの不存在を確かめ、所有不明の物を推測で削除していない。SSH rule→subnet→VPCの順に撤去し、独立した最終読取りで次を確認した。

| resource区分 | 最終件数 | 今回比較前／最初のA開始前との比較 |
| --- | --- | --- |
| VM／disk／address／snapshot／resource policy | 各0 | 取得項目が両baselineと一致 |
| network | 1 | 同上 |
| subnet | 42 | 同上 |
| firewall | 4 | 同上 |
| route | 43 | 同上 |

**15:17:00 JSTに撤去・九区分照合を完了。** これは取得resource・項目の比較であり、全account・全serviceの無影響証明ではない。既定network／rule、Compute API、既定生成物は保全。新設VM・disk・外部IPの残存なし。実請求額の確定照合は未実施で、請求画面でUS$0を確認したとは報告しない。

## 現在地と次の判断

今回承認された一回比較は終了。公開pilotはHOLD、GCPのguest準備・SSH・隔離実XDP・BAN／解除／失効／restartは未実施。公開Bの確認へ進める状態にも達していない。

次に同じ作成を漫然と繰り返さない。再開するなら、保存したrequest／503の情報から追加の判断材料を得るか、別image・machine・region等を比較する理由と具体範囲を先に整理する。追加作成や時間枠の再開始は今回の承認に含めない。製品コードや安全条件を変えてCompute APIの失敗を回避しようとしない。

## 証拠

private artifact: `artifacts/gcp-a-zoneb-977x9lrm/`。

- [sessionと元の期限](../../artifacts/gcp-a-zoneb-977x9lrm/session.json)、[最終結果・九区分照合](../../artifacts/gcp-a-zoneb-977x9lrm/final-summary.json)。
- [target要求](../../artifacts/gcp-a-zoneb-977x9lrm/target-request.json)、[失敗receipt](../../artifacts/gcp-a-zoneb-977x9lrm/target-receipt.json)、[HTTP限定抽出](../../artifacts/gcp-a-zoneb-977x9lrm/creation-http-summary.json)。auth header・token・元のlog全体を複製しない。
- [失敗後VM](../../artifacts/gcp-a-zoneb-977x9lrm/after-failure-instances.json)、[disk](../../artifacts/gcp-a-zoneb-977x9lrm/after-failure-disks.json)、[operation](../../artifacts/gcp-a-zoneb-977x9lrm/after-failure-operations.json)、各区分のbefore／final JSON、所有ID照合・削除のcleanup JSONL。
- [作成helper](../../artifacts/gcp-a-zoneb-977x9lrm/phase.py)、[撤去helper](../../artifacts/gcp-a-zoneb-977x9lrm/cleanup.py)。zone変更・原clock／予算の同値・request排他作成・公開phase拒否・固定SSH source・service accountなしを静的確認した。VM上の回帰検証とは区別する。

[試験履歴](../local-ban/TEST_HISTORY.md)へ戻る。
