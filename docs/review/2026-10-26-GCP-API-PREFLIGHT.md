# GCP準備：API有効化前の読取り確認

> 後続: 規約表示を踏まえた本人承認後に[APIを有効化し、事後表示・割当・作成前見積りを確認した](2026-10-26-GCP-API-ENABLED.md)。以下は有効化前で停止した時点の記録として保全する。

2026-10-26 JST。対象は本人指定の`veil-x-tests`のみ。最終ゴールは公開サイトでの限定的な試験導入。今回はクラウドVM試験の前提確認と、その経過をREADMEから辿れる形にすること。

**API有効化は未実施。VM・VPC・firewall・public IPの作成、公開、課金設定変更、手動のIAM権限追加も行っていない。VM試験の受入れ結果ではない。**

## 承認と確認した範囲

本人からCompute Engine APIだけの有効化と、既存resource・quota・見積りの読取り確認について承認を受けた。自動生成され得るservice account／既定networkの確認を含む一方、追加の規約同意・課金accountリンク・権限要求が必要なら止める条件。

| 確認 | 表示された結果と限界 |
| --- | --- |
| project | welcome画面でproject ID `veil-x-tests`を再確認。別projectへの操作なし |
| IAM | 「Google提供のロール付与を含める」を表示条件として選び、所有者の行を確認。ロール編集やアクセス付与はしていない。表示filterはIAM変更ではない |
| service account一覧 | 有効化前に「表示する行がありません」。この一覧だけでGoogle内部の全service identityの不在を証明しない |
| VPC一覧 | 読込み後に「データを読み込めませんでした」。networkが存在しないとは判定できず、変更前後の完全比較は未達 |
| API | `compute.googleapis.com`、Compute Engine API、「このAPIを有効にする」を確認。クリックしていない |
| 規約表示 | 「このプロダクトを使用することで」Google Cloud Platformの利用規約へ同意する旨を表示。API有効化の承認とは別に、この表示を踏まえた確認を依頼した |

規約が既存契約から変更されたと確認したわけではない。画面が使用に伴う同意を示すため、同意の扱いを推測して実行しない。本人の追加回答まではAPI有効化前で停止する。

## 未完了と再開位置

規約表示を踏まえた本人確認後、同じprojectとAPIを照合してから一度だけ有効化し、成否を表示で確認する。service account／IAM／networkの事後表示を読み、既定生成物を手動作成したものと分けて記録する。VPCの事前一覧は取得できていないため、事後に見えるnetworkを無条件に「今回生成」と断定しない。

quota、既存VM・disk、課金リンク、作成見積りの最終確認は未完了。課金リンクや新規resource作成を読取り確認の延長で実施しない。[構成・費用・復旧案](../local-ban/GCP_TRIAL_PLAN.md)に従い、VM作成は具体的な別承認の対象とする。

## 保全・復旧

今回cloud resourceの変更はなく、Pi・hp・router、Macのnetwork設定にも操作していない。秘密鍵・tokenの取得、service account key作成、支払方法・IAM roleの編集なし。アカウント識別子等をこの報告へ転記していない。

今後APIを有効化した後の復旧案は、新VMや依存利用がないことを確認して当該APIだけ無効化すること。依存APIの強制無効化や自動生成resourceの削除を含めず、無効化だけで全resourceが元どおりになるとは扱わない。

[試験履歴](../local-ban/TEST_HISTORY.md)へ戻る。
