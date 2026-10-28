# GCPのeBPF・XDP対応と操作できる層の公式確認

2026-10-26 JST。Sol／High、分担なし。本人指示により、公式資料と現在のloaderを確認した。最終ゴールは限定的な公開サイト試験導入の可否判断。今回の完了条件は、GCPが試験候補となる根拠・制約と、作成503との関係を整理すること。

**Cloud RunのコンテナはeBPF非対応。一方、今回最初から要求していたCompute EngineのLinux VMは、ゲストOSの管理権限があり、Google公式の仮想NICドライバーにもXDP対応が記載されている。Compute Engineを選んだこと自体を誤りとする根拠はなく、条件付きの試験候補。ただし、今回のイメージ・カーネル・仮想NICで製品が動くことは未検証。**

## 公式で確認した境界

| 層・サービス | 確認したこと | 今回の判断 |
| --- | --- | --- |
| Cloud Runの管理コンテナ | eBPFは無効・非対応。コンテナ内rootでもホスト権限を得るわけではなく、privileged modeもない | 製品のXDP執行先として不採用。今回利用していない |
| Compute EngineのゲストOS | 公開Linuxイメージで管理者操作が可能。公式はrootの直接SSHよりsudoを推奨 | 管理可能なLinux VMとして試験候補 |
| ゲストカーネル | 要件に沿った独自カーネル・OSの利用も公式に説明されている | 必要なカーネル機能や権限はゲスト側で確認する。今回はカーネル変更なし |
| ゲストの仮想NIC | VirtIO-Net／gVNICがあり、Google公式gveドライバーにXDPの機能・条件が記載されている | 実際のdriver・mode・設定が合うかは別に確認する |
| Googleのホスト・物理NIC | VMはGoogleのKVMベースのhypervisor上のゲストとして動く | ゲストXDPの成功を物理NICへの装着やホスト側の制御・性能の証明にしない |

根拠は[Cloud Run runtime contract](https://docs.cloud.google.com/run/docs/container-contract#container_security_restrictions)、[Compute Engineの管理者操作](https://docs.cloud.google.com/compute/docs/connect/root-ssh)、[独自OS・カーネルの要件](https://docs.cloud.google.com/compute/docs/images/building-custom-os)、[gVNICの利用](https://docs.cloud.google.com/compute/docs/networking/using-gvnic)、[仮想化の層](https://docs.cloud.google.com/compute/docs/instances/nested-virtualization/overview)。資料上の機能と、未作成VMでの実測・実権限は区別する。

## XDPには対応条件がある

[Google公式gveドライバーのXDP節](https://github.com/GoogleCloudPlatform/compute-virtual-ethernet-linux#xdp)にはPASS／DROP等の対応と、queue形式別の導入versionが記載されている。nativeのdriver modeではqueue数、header-data split、RX buffer長の条件がある。gVNICを選べば無条件で使える、という意味ではない。この条件を満たすためのdriver更新・queue変更を今回実行していない。

[Linux公式のAF_XDP説明](https://www.kernel.org/doc/html/latest/networking/af_xdp.html)は、generic XDPのSKB経路とdriver対応の経路を区別している。これはgenericとnativeの区別の根拠として使い、現在の製品がAF_XDP socketやzero-copyを使うという説明にはしない。genericで機能確認できても、nativeや物理NIC offloadの性能を立証したことにはならない。

現在の製品は[local_ban/kernel.rs](../../veil-node/src/local_ban/kernel.rs)でLinux 5.9以上とFD所有のlinkを要求し、`xdp.attach(interface, XdpFlags::empty())`を呼ぶ。**genericを明示する実装でも、native失敗時に安全なgeneric再試行を保証する実装でもない。** 実modeと既存XDP非置換・FD終了時の解除を新VMで確認する必要がある。今回はloader・依存・objectを変更していない。

E2でnested virtualizationを使えないという公式制約は、VM内でさらにVMを動かす機能の制約。今回のゲストカーネルでeBPFを使うためにnested virtualizationは必要なく、この制約をeBPF禁止と取り違えない。

## VMを落として作り直す案とログ

[既存監査ログの分析](2026-10-26-GCP-503-REQUEST-ANALYSIS.md)で、三件の作成要求は503／UNAVAILABLE、15:30:22 JSTの最終resource確認ではVM0・disk0だった。停止・再起動する試験VMは成立していない。ここで再作成と呼ぶ操作は、実際には新しいinsert要求になる。今回は現在のresource一覧を再照会していない。

ゲストが成立していないため、今回の製品・カーネル・driverログはない。一方、Google側の監査ログは取得済みで、三件の時刻・zone・status・記録された権限・選択request項目を保存している。「構築されていないからログが一切取れない」わけではない。

製品のeBPFロードには到達していないため、今回の503をeBPF禁止の結果と結び付ける証拠はない。Compute Engineの適合性と、今回の作成失敗の内部原因は別の判断項目として残す。一般的なXDP対応資料で503が解消したとは判定しない。

## 次に進む最小の順序

公式資料からは、Compute Engineの候補を捨てたり、今回のために別の有料supportを契約したりする必要は認められない。一方、同じ失敗を反復する根拠も増えていない。

次に作成を再開するなら、まず保護対象候補一台だけで作成・SSH・kernel／driver／BPF機能・権限を確認し、隔離vethで製品のload・attach・detachを確認。その後、専用VMの仮想NICで既存programを置換せず装着・解除と管理復旧を確認できた時点で、二台目と公開受入れへ進む。generic／nativeやNIC設定の変更が必要なら具体的な差分と復旧方法を先に示す。native対応のためだけに時間を使い続けず、今回の機能受入れに必要なモードで採否を決める。

これは次回計画への推奨であり、新しい作成要求・課金・公開Bの実行承認ではない。元のUS$1・絶対期限を延長・再設定していない。既存資料にあった「新VMでkernel／NICを確認する」条件を、具体的な一次資料とloaderの境界まで明確にした。GCPで製品の動作証拠が増えた意味ではない。

問い合わせ可能な窓口が得られた場合の追加質問は、「Compute Engineの指定LinuxゲストでBPF_PROG_LOAD、BPF_XDPのFD所有link、BPF mapと必要なLinux権限を利用できるか。指定kernel／VirtIOまたはgVNICでgeneric／native XDPにどの制約があるか。これは既存三件のinstances.insert 503の内部reason確認とは分けて回答してほしい」とする。既存の承認対象文面を上書きせず、今回は追加質問も送信していない。

残りはVM準備、GCP隔離実XDP、承認B後の限定公開受入れ、撤去・導入判定の四工程。[六つの受入れ条件](../local-ban/DELIVERY_GATE.md)を満たした環境を限定試験導入候補と判定する。今回の文書調査は完了、実環境の受入れと503解消は未完了。

[試験履歴](../local-ban/TEST_HISTORY.md)へ戻る。
