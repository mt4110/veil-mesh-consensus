# GCPの請求・CLI状態と、AWS候補の公式確認

2026-10-26 JST。Sol／High、分担なし。本人の請求停止・名前解決・静的IPへの疑問と、AWSへの切替案を受け、読み取りと公式資料の確認を行った。最終ゴールは限定公開試験導入の可否判断、今回の完了条件は現障害と環境選択の判断材料を明確にすること。追加VM作成・契約・支払い・設定変更はない。

## 今回確認したGCPの状態

20:47 JSTのCLI照会で次を確認した。[private記録](../../artifacts/gcp-billing-status-g_i195d2/summary.json)。請求先識別子や個人情報を本文へ転記しない。

| 項目 | 観測 |
| --- | --- |
| project | `veil-x-tests`、lifecycleState=ACTIVE |
| 請求の紐付け | 請求先あり、billingEnabled=true |
| 請求先状態 | open=true |
| Compute API | 有効 |
| 一覧API | 正常応答、VM0・disk0 |
| CLI default project | 未設定。作業では対象projectを明示しており、未設定自体を今回の失敗原因にしない |

さらに`gcloud info --run-diagnostics --anonymize --project=veil-x-tests --quiet`を一回実行し、returncode0、Reachability CheckとNetwork／Property diagnosticの合格を確認した。[限定した診断結果](../../artifacts/gcp-billing-status-g_i195d2/cli-diagnostics-result.json)。これで全Google内部サービスやVM作成経路の正常性を証明した意味ではない。

[Googleの請求状態確認手順](https://docs.cloud.google.com/billing/docs/how-to/verify-billing-enabled)と照合すると、通常の請求停止・紐付けなしを示す状態ではない。ただし支払方法の画面・カード会社の内部制限・個別取引を確認していないため、「未払いは全くない」「次の決済が必ず通る」とは断定しない。支払い、カードの変更、請求停止、予算設定、契約変更は行っていない。

公開の[Google Cloud Service Health](https://status.cloud.google.com/)は「No broad severe incidents」、ページ更新は2026-10-26 04:28 PDT（20:28 JST）だった。これは広範囲の重大障害の告知状況であり、特定project・region・作成経路の不具合がないことの保証ではない。以前のSupport概要の表示とも区別する。

## 503、DNS、静的IP、低レイヤーの区別

- 今回はGoogleの監査にinsert要求とcode14が記録され、VM作成以外のAPIも応答している。手元の名前解決が失敗してGoogleへ届かないという説明とは合わない。503はサービス利用不可のHTTP応答で、Google内部の依存先障害等を含み得るが、具体的な内部原因は不明。[公式error catalog](https://docs.cloud.google.com/compute/docs/reference/rest/v1/errors)。
- 外部IPv4はephemeralでもstaticでも利用でき、静的IPの予約は必須ではない。今回はSSH接続やIP固定の前にVM作成が失敗している。[外部IPの公式説明](https://docs.cloud.google.com/compute/docs/ip-addresses/configure-static-external-ip-address)。
- VMを作る要求に、製品binary・eBPF object・独自アルゴリズムを含めていない。guestのeBPFロードにも達していない。秘密のアルゴリズム規制を今回の原因とみなす根拠はなく、Google公式gveのXDP対応とも区別する。[低レイヤー対応確認](2026-10-26-GCP-EBPF-XDP-OFFICIAL-FEASIBILITY.md)。
- 残る候補は同regionの容量、project固有の内部状態、machine・期限・保護設定等を扱うprovisioning経路の不具合など。一般的な権限・quota・請求のチェックだけでは内部条件を全て排除できない。候補を断定したり、根拠なく同条件の再作成を繰り返したりしない。

## Cloudflareで置き換えられる範囲

公開サイトへのHTTP要求を遮断する目的なら、[Cloudflare WAFのcustom rules](https://developers.cloudflare.com/waf/custom-rules/)でBlock等が可能。適用先・契約条件・正常利用者への影響は、変更前に別途確認する。今回、既存zoneやruleを変更していない。

一方、[Workersの実行モデル](https://developers.cloudflare.com/workers/reference/security-model/)はV8 isolateとsandboxで、Linux管理者として今回のXDP製品を装着する実行先ではない。Cloudflare Containersは独自kernelのmicroVMを使うが、[公式の経路説明](https://developers.cloudflare.com/containers/concepts/architecture/)では受信はWorker経由となる。必要なBPF権限・link・管理NIC操作の提供までは確認できず、同一構成の公開XDP試験の単純な代替にしない。「Cloudflareでは何もできない」とも「ContainersもeBPF禁止」とも断定しない。

## AWSの候補確認とウォーミングアップの仮説

本人からAWSの利用案が出た。次の候補は管理可能なLinuxの**EC2**であり、LambdaやFargateのsandboxへ置き換える案ではない。

[AWS公式のENA説明](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/enhanced-networking-ena.html)と[driverの診断資料](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/troubleshooting-ena.html)を確認。後者にはXDP_DROP／PASS等のcounterも記載され、LinuxゲストのXDP試験候補とする根拠がある。特定AMI・kernel・instance typeで製品のFD所有linkとmodeが使えることは未確認で、AWSでも一台の適合確認を先に行う。

MacのAWS CLIは既に2.36.50が導入されていた。今回確認したのはversionだけで、credential fileを読まず、AWSへ認証・API照会・作成を行っていない。既存アカウントの有無を本人へ確認中で、対象account・region・AMI・費用・期限・撤去の具体案は未確定。GCPの作成承認をAWSへ流用しない。

新規アカウントの有効化待ちや作成後のeventual consistencyは実在するが、AWSでも同じHTTP番号だっただけで「ウォーミングアップ」と結論しない。[EC2の公式エラー一覧](https://docs.aws.amazon.com/ec2/latest/devguide/errors-overview.html)はPendingVerification、OptInRequired、InsufficientInstanceCapacity、ServiceUnavailable等を区別する。AWSで実行する場合はHTTP番号だけでなくservice error code・RequestID・停止段階を記録する。AWSの候補を使ってGCPの原因が証明されたとは扱わない。

## 現在の推奨と残り

GCPの追加作成は保留を維持する。AWSの既存accountが使えるなら、一台の適合確認案を具体化する価値がある。新規契約が必要なら、その導入負担も含めて判断し、黙って登録・課金しない。

残りは、管理可能なLinux VMの準備、一台での隔離XDP／NIC適合確認、承認後の限定公開受入れ、撤去・導入判定。providerが変わっても[六つの受入れ条件](../local-ban/DELIVERY_GATE.md)を維持する。今回の状態確認は終了したが、GCPの503解消・AWS環境準備・公開受入れは完了していない。

[試験履歴](../local-ban/TEST_HISTORY.md)へ戻る。
