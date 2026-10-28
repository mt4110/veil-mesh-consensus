# GCP短期VM試験案：veil-x-tests

2026-10-26。本人の既存GCPを使う方針へ変更し、DigitalOceanの新規契約は行わない。対象projectは本人指定の`veil-x-tests`だけ。[ゴールと六つの受入れ条件](DELIVERY_GATE.md)は維持する。

**Compute Engine API有効化後、承認Aと承認済み別zone一回比較を実施したが、VM作成POSTは二zoneともHTTP503で未成立。公開・課金設定変更・手動のIAM変更は未実施。新設network等は撤去し、既存resourceの9区分は両baselineと一致。** [別zone比較の実行結果](../review/2026-10-26-GCP-A-ZONE-B-COMPARISON.md)を最新状態として参照し、下の準備時の候補・観測と区別する。

追記: 規約表示を踏まえた本人承認後に有効化した。[有効化後の権限・network・割当・見積り](../review/2026-10-26-GCP-API-ENABLED.md)、[その前の停止記録](../review/2026-10-26-GCP-API-PREFLIGHT.md)、[全体の試験履歴](TEST_HISTORY.md)を参照。

**作成前の具体案は[二台構成・限定公開・全費目・期限付き撤去](GCP_TWO_VM_EXECUTION_PLAN.md)へ固定した。** 以下の候補表は準備時の比較記録。新案はMacを正常clientとする二つの/32、専用FQDNのDNS-01、証拠退避と新設VM／diskの期限付き自動削除を提案する。従来のSTOPだけの案からの変更。Aは承認済みで、[認証待ちの後](../review/2026-10-26-GCP-A-PREPARATION.md)に実施したが、上記503で停止・撤去した。公開Bは未承認。

## 現在確認できたこと

- browserで`veil-x-tests`のproject IDを確認。初回画面には以前の別projectを指すリンクが残っていたため、再読込みで一致させてから操作した。別projectへ操作していない。
- 初回のCompute概要の0は、API無効時のresource不存在の証明にしなかった。有効化後のVM一覧は空であることを確認。他の全resourceがないと判定したわけではない。
- 既存請求先accountの管理画面でprojectのリンク済み表示を確認。適用料金・残高・利用可能credit・税・総額の最終確認は未完了。支払方法・請求先リンク・IAM roleの手動変更なし。
- 既定service accountにEditor、Googleのservice agent、default VPCと既定四ruleを事後に確認。VMへ既定accountを付けず、新規custom VPCを使う案を維持する。既定生成物を無断編集・削除しない。
- Macに`gcloud`はあるが、現credentialのrefreshは`invalid_grant`で失敗した。CLI認証を勝手に更新したり、tokenを取り出したりしていない。

## 承認済みAPI有効化の範囲（実施済み）

`veil-x-tests`の`compute.googleapis.com`を有効化し、一覧・quota・作成画面の見積りを読み取り確認した。**この承認にVM・disk・public IPの作成や課金accountの新規リンクは含めない。**

Googleは有効化時にCompute Engine default service accountやservice agentを自動生成し、組織policyによっては既定roleを付与する。default VPC・subnet・firewall等が生成される場合もある。これらをbefore／afterで確認する。手動のEditor付与、組織policy変更、既存networkへのrule追加はしない。[service account](https://docs.cloud.google.com/compute/docs/access/service-accounts)、[default networkの生成](https://docs.cloud.google.com/architecture/blueprints/security-foundations/preventative-controls)

復旧は、新VMや依存利用がないことを確認して当該APIを無効へ戻す案。依存サービスの強制無効化はしない。自動生成されたaccount／networkを無断削除せず保全し、残存を報告する。API無効化だけで生成前の全resource状態へ戻るとは説明しない。別の規約同意・課金accountリンク・追加権限の要求が出たら、そこで止めて本人へ確認する。

## API有効化後に見積るVM案

| 項目 | 候補 |
| --- | --- |
| project | `veil-x-tests`をすべての操作で明示し、共有CLIのdefault projectを変更しない |
| VM | `veil-ban-target-test`と`veil-ban-client-test`、最大二台。既存同名なら保全して中止 |
| machine | `e2-small`、2GiB RAM。shared-coreで、専用2core相当の性能とは扱わない |
| OS | `ubuntu-os-cloud`のUbuntu 26.04 LTS x86_64。公式familyは`ubuntu-2604-lts-amd64`。作成時の具体imageとhashを記録 |
| 地域 | 料金例を確認した`us-central1`を候補とし、zone・在庫・quotaは確認待ち。変更時は見積りも更新 |
| disk | 新規boot disk各20GiB、standard persistent disk案。追加disk／snapshot／backupなし |
| network | 専用custom VPC・単一subnet案。既存default networkや既存firewallを改変しない。公開IPv4各一つ、reserved IP／Cloud NAT／LBなし |
| VMの権限 | アプリによるGoogle API利用は不要なため、VMへservice accountを付けない案。project全体への権限付与やkey file生成をしない |
| 時間 | 最大四時間。Compute Engineの時刻指定による自動停止を候補にする。証拠を失わないため自動削除は使わず、期限後のdisk等の課金にも対処 |

GCPのUbuntuは`linux-image-gcp`を使い、自宅のgeneric kernelと同一ではない。VirtIO／gVNIC、実ingress、XDPの装着・解除・source／destination tupleを新VMで確認する。対応が確認できなければ、権限や安全条件を緩めず公開前に止める。[公式OS一覧](https://docs.cloud.google.com/compute/docs/images/os-details)、[network概要](https://docs.cloud.google.com/compute/docs/networking/network-overview)

## 費用

公式料金例では`us-central1`のe2-smallが一台$0.016752855／時。二台・四時間のVM部分は計算上約$0.134。通常VMの使用中外部IPv4は一つ$0.005／時なので、二つ・四時間は$0.04。**VMとIPv4の合計例は約$0.174で、disk・通信・税は別。** DigitalOceanの約$0.15という数字をGCPの総額に流用しない。[VM料金](https://cloud.google.com/products/compute/pricing/general-purpose?hl=en)、[IPv4料金](https://cloud.google.com/vpc/pricing)

利用料上限案US$1を維持するが、これをプロバイダーの強制課金停止機能とは扱わない。projectの適用通貨・割引・課金状態と見積りを読み、作成前に金額を確定する。無料枠・creditを使えると仮定しない。APIだけを有効化しても、こちらからVM稼働は開始しない。

Compute Engineは指定時刻で停止／削除できるが、発動には遅延があり、停止後もdisk等の課金が残る。停止を全課金の終了と報告しない。[VM時間制限](https://docs.cloud.google.com/compute/docs/instances/limit-vm-runtime)

## 試験と後片付け

BAN対象一件、TCP18443、TTL30秒、容量256、daemon180秒／HTTPS240秒、再起動でBANが消える契約を維持する。公開前の隔離検証と、明示解除・自然失効・通常daemon restartの短いcaseを先に固定する。

管理sourceと独立正常clientの同じ入口への到達を設計する。正常clientのpublic sourceを追加許可する必要があれば、以前の外部source一件許可からの拡張として、実値・用途・戻し方を示して別承認を得る。private NICだけの対照をpublic ingressの正常系の証明にしない。通常のCloudflare proxy／Tunnelを経由して利用者の代わりに中継元をBANしない。

終了時は新unit停止・XDP解除、試験port閉鎖、証拠のMacへの退避とhash照合。その後、新規VM・boot disk・専用firewall／subnet／VPC等をID・名前・所有manifestで列挙し、具体的な対象・不可逆性への承認後に削除する。project自体、既存resource、既定service accountを削除対象へ含めない。残る課金対象がないことを確認し、不明なら未完了として報告する。

本人の最新指示により携帯テザリングを以後使用しない。自宅の追加WAN試験は保留し、再開指示がない限り行わない。このGCP準備を実試験の成功には数えない。Piと自宅networkの変更なし。本人の依頼どおり、今回使ったGCP確認用タブは作業終了時に閉じる。
