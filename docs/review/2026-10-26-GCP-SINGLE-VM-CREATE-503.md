# GCP一台適合確認：Ubuntu 24.04でも作成503、撤去完了

2026-10-26 JST。Sol／High、分担なし。[一台試験の具体案](../local-ban/GCP_SINGLE_VM_COMPATIBILITY_PLAN.md)を提示後、本人の「推奨で」を、新しい一台・一回・最大60分・追加US$0.20・対象の期限付き削除と撤去への承認として受領した。

**Ubuntu 24.04の固定imageへ変更した一回のVM作成もHTTP503／UNAVAILABLEで失敗。再送なし。今回新設したSSH rule・subnet・VPCは撤去し、20:05:54 JSTに九区分の選択項目が開始前と一致した。VM・disk・予約addressは0件。guest SSH・eBPF・XDPは未実施で、適合確認は未達。**

最終ゴールは小規模公開サイトへの限定試験導入の可否判断。今回のマイルストーンは一台のVMでkernel／NIC／XDP装着解除を確認することで、作成段階から進めなかった。作成エラーと、製品が動作不可だったという結果を区別する。公開B・二台目・DNS・証明書・携帯テザリングは使っていない。

## 承認対象と実行時刻

| 項目 | 実際の内容 |
| --- | --- |
| project／zone／machine | `veil-x-tests`／`us-central1-b`／e2-small、STANDARD |
| VM／boot disk名 | `vlb-one-20261026-target`、新規20GiB pd-standard、autoDelete=trueの要求 |
| sourceImage | `ubuntu-os-cloud/ubuntu-2404-noble-amd64-v20261024`、事前image ID `4575951068918267275`、READY、X86_64 |
| 作成時の安全条件 | service account・scopesなし、Shielded三項目有効、project-wide SSH鍵遮断、専用公開鍵のみ、絶対期限DELETE |
| ネットワーク | 専用custom VPC、`10.77.25.0/28`、IPv4-only、管理元一件/32から専用tagへのTCP22だけ。18443公開なし |
| T0 | **20:04:28.715306 JST**、最初の新設操作直前に一度だけ記録 |
| VM insertの失敗 | **20:05:03 JST**。一回のみ |
| 要求したautoDELETE | 20:49:28.715 JST。VMが成立していないため実VMへの登録はない |
| 計画の全体上限 | 21:04:28.715306 JST。失敗で試行を終え、残時間を再試行に使わない |
| 撤去・照合完了 | **20:05:54.554106 JST** |

上限は今回の新しい承認範囲。元のAのT0・期限を改変していない。実VM・disk・外部IPは成立していないが、実請求を照合したわけではなく、確定費用を0と断言しない。今回の費用上限をUS$100へ広げない。

## 作成の結果と得られたログ

新設操作はnetwork、subnet、SSH ruleが成功し、VM insertが失敗した。[実行記録](../../artifacts/gcp-single-live-yjcacd_o/)にrequestとreceiptを分離保存し、同じtarget requestファイルを排他的に作成することで自動再送を行わない構造にした。実行helperのexit 1は作成失敗の結果で、復旧未完了を意味しない。撤去の結果を独立記録した。

CLIのstderrは従来と同じ`Internal error. Please try again or contact Google Support.`。この要求のSDKログから、VM insert POSTのHTTP503を抽出した。[最終要約](../../artifacts/gcp-single-live-yjcacd_o/final-summary.json)には限定した観測だけを保存し、原HTTP headerやcredentialは複製しない。

既存Logging APIから該当要求を読み取り、次を確認した。

- `2026-10-26T11:05:03.315465Z`、`v1.compute.instances.insert`、status.code **14（UNAVAILABLE）**。
- 記録された七権限は全てgranted。これで全ての内部認可を証明したとは扱わない。
- sourceImageは実際に**24.04の固定image**。boot disk一枚・20GiB・autoDelete=true、e2-small、Shielded、期限付きDELETEも取得項目と整合。
- 監査insertIdはprivate記録に保全。Compute API requestId／operation IDと混同しない。

最初のCLI projectionではdisks配列内の項目が出ず、局所的な照合assertが失敗した。欠落を実要求の欠落とは解釈せず、同じ監査エントリのdisksだけを追加取得して確認した。これはログの読み取りと解析の修正で、VM insertの再試行ではない。最初の選択ログも残している。[選択した監査情報](../../artifacts/gcp-single-live-yjcacd_o/target-audit-selected.json)・[disksの追加選択](../../artifacts/gcp-single-live-yjcacd_o/target-audit-disks-selected.json)。

## 撤去と保全

| 今回作成したもの | 作成成功時のID | 終了時 |
| --- | --- | --- |
| VPC `vlb-one-20261026` | `1528244010843540818` | 不在 |
| subnet `vlb-one-20261026-us-central1` | `3324233904775400739` | 不在 |
| SSH rule `vlb-one-20261026-ssh` | `6887136639491149114` | 不在 |
| VM／boot disk | 作成失敗、所有IDなし | どちらも不在を確認 |

削除前に所有ID・description・依存を確認し、今回の三resourceだけを撤去。未知の同名物・依存を見つけた場合は削除しないガードを維持した。

九区分の前後は、instances0、disks0、networks1、subnets42、firewalls4、addresses0、routes43、snapshots0、resource policies0。**件数だけでなく、取得した選択項目を正規化して全九区分が一致**した。[撤去と前後照合](../../artifacts/gcp-single-live-yjcacd_o/cleanup-result.json)。これはaccount全サービス・全属性・請求の完全照合ではない。

元のBFT、既存artifact、試験鍵、GCP既存resourceとIAMを保全した。Pi・hp・router・Macのネットワーク設定には操作していない。今回はbrowserを使わず、新しい作業用タブはない。

## 判断・残り・終了条件

**今回の一回は終了。GCPでの作成反復はここで保留する。** 26.04から24.04へimageを替えるだけでは解消しなかった。別時刻・名前・期限等も変わるため、厳密な一変数実験ではない。二イメージ・同region二zoneの失敗から、どのGCP内部条件が原因かを断定しない。

eBPFをまだロードしていないため、この503をeBPF禁止の結果とは扱わない。[公式仕様に基づくCompute Engineの条件付き適合性](2026-10-26-GCP-EBPF-XDP-OFFICIAL-FEASIBILITY.md)は候補の根拠として残り、今回の実適合性は未検証のまま。製品コードを直せば解決する証拠も得られていない。

次の判断に必要なのは、再作成を増やすことではなく、作成経路の具体的な新情報または別の承認済み準備方法。元の一般的な503の調査を自動再開したり、予算を再設定したり、有料supportを契約したりしない。

残りはVM準備、GCP隔離実XDP、承認B後の限定公開受入れ、撤去・導入判定の四工程。今回の撤去は完了したが、将来の公開試験の撤去と受入れまで終わった意味ではない。最終ゴールは[六つの公開受入れ条件](../local-ban/DELIVERY_GATE.md)と保全・費用確認を揃えて限定試験導入の可否を判定すること。環境が用意できていないため、総完了日は未定。

[試験履歴](../local-ban/TEST_HISTORY.md)へ戻る。
