# クラウド・エッジ環境の過去の構成案と確認状況

[README](../README.md) · [English README](../README_EN.md) · [公開コード・試験の確認状況](VALIDATION_STATUS.md) · [クラウド試験要約](review/2026-10-27-CLOUD-VALIDATION-SUMMARY.md)

2026-10-27訂正。開発終了・本番導入非推奨。本書は導入成功を確認した手順書ではありません。旧版にあった「Linux VMならそのまま動作」「UDPを許可するだけでメッシュ完成」「3社共通の本番systemd手順」は裏付けがなく、実行可能な導入案内として撤回します。

## 1. プラットフォーム別の確認状況

AWS・GCPの実測は、公開mainと同じではない後続ローカル版 `veil-local-ban` の記録です。単体hostの隔離XDPと実NIC適合を分け、公開mainの分散防御全体の合格とは扱いません。

| プラットフォーム | 本実装の確認状況 | 過去の構成案 | 検証上の制約 |
| :--- | :---: | :--- | :--- |
| **AWS** (EC2 / EKS) | **試したEC2構成では実NIC装着失敗** | 過去の構成案：ENA XDP | 隔離XDPは合格。ENA／MTU 9001で装着拒否。EKS・別構成は未検証。 |
| **GCP** (Compute Engine / GKE) | **試したCompute Engine構成では実NIC装着失敗** | 過去の構成案：gVNIC XDP | 隔離XDPは合格。VirtIO-Net／MTU 1460でEINVAL。gVNIC・GKE・別構成は未検証。 |
| **Microsoft Azure** (Azure VM / AKS) | **未検証** | 過去の構成案：MANA / Accelerated Networking | 本実装の導入成功は未確認。 |
| **Cloudflare** (WAF / Workers / Magic Transit) | **連携未検証** | 前段センサー・API遮断同期・オリジン保護 | 対応する連携実装と受入れ記録は未確認。構成案を連携対応の根拠にしない。 |
| **Vercel** (Functions / Edge Network) | **連携未検証** | フロントエンドと別Linuxバックエンド | 対応する連携実装と受入れ記録は未確認。別ホスト構成の提案を連携成功の根拠にしない。 |

## 2. kernel・NIC・権限と公開版の制約

kernel操作の権限があることだけでは、本実装の稼働は保証できません。driver、NIC、MTU、offload、attach方式、object／source／ABI、PKI、通常daemonのruntime、停止復旧を含める必要があります。Linuxの版番号だけを汎用対応の根拠にしません。

公開mainのXDPはexact IPv4 HASHで、kernel TTLの期限fieldはありません。Rust eBPFソースにはLSM hookがあり、loaderにも任意のattach経路がありますが、同梱C版objectによるLSMの本番受入れやアプリ秘密鍵消去は未実証です。`lsm=bpf` を追加するだけでステルスGCや実アプリ保護が成立するとは案内しません。

旧版のGRUB書換え・reboot・NIC MTU変更例は、読者の環境で安全性・復旧・動作を確認した手順ではありません。本書はそれらの実行を推奨しません。通常daemonはXDP attach失敗でもQUIC-onlyで継続するため、起動成功を防御開始と取り違えないでください。

<a id="3-aws-amazon-ec2--vpc-への実践導入手順"></a>

## 3. AWS：過去の構成案と試験結果

旧案はEC2のENAへのXDPとQUICを組み合わせるものでした。セキュリティグループのUDP許可だけで、安全なPKI・node ID対応・選挙・提案受付・合意・kernel適用が成立するわけではありません。

後続ローカル版の一台試験では、隔離vethの実XDPは合格し、ENA／MTU 9001の実NIC装着は拒否されました。kernelログの3498は当該guestの上限で、全ENA構成への推奨MTUではありません。MTU1500への変更や別modeなら成功するという実測もありません。EKSや他構成は未検証です。[公開用要約](review/2026-10-27-CLOUD-VALIDATION-SUMMARY.md)を参照してください。

<a id="4-gcp-google-compute-engine--vpc-への実践導入手順"></a>

## 4. GCP：過去の構成案と試験結果

旧案はgVNICへの装着を想定しましたが、後続ローカル版で実際に試したNICはVirtIO-Net／MTU 1460です。隔離vethの実XDPは合格し、実NICへのFD所有link作成はEINVALで失敗しました。原因はこのエラーだけでは確定できません。

gVNIC、GKE、別driver・mode・offload構成は未検証です。旧案のmachine typeとgVNICの組合せ、`ens4` という名前やfirewall設定例だけを、本実装の導入成功の根拠にしません。

<a id="5-microsoft-azure-azure-virtual-machines--vnet-への実践導入手順"></a>

## 5. Azure：未検証の構成案

Azure VM／AKS、MANA／Accelerated Networkingへの本実装の装着・運用受入れは未検証です。`eth0` というinterface名を普遍的な条件としません。NSGでUDPを許可したり、rootのsystemd unitを置いたりすれば分散防御が完成するという旧案は、成功した手順ではありません。

<a id="6-cloudflare--vercel-を使う場合のハイブリッド実践構成"></a>

## 6. Cloudflare／Vercel：連携未検証の構成案

CloudflareのL7観測や遮断APIとオリジンXDPを組み合わせる案、Vercelのfrontendと別Linux backendを分ける案がありました。公開コード・手元の実装と試験文書を調べた範囲では、対応する連携実装・受入れ記録は確認できませんでした。「連携対応」「最も強力」などの推奨表現は撤回しています。

Cloudflareは元の訪問者IPをHTTPヘッダーで渡すことを[説明しています](https://developers.cloudflare.com/support/troubleshooting/restoring-visitor-ips/restoring-original-visitor-ips/)。この情報はアプリ層で検証するもので、XDPが見るL3送信元を利用者IPへ書き換えるわけではありません。proxyのIPを元利用者としてBANすると、他の利用者も巻き込み得ます。

各サービス自体の機能と本作の連携成立は別です。Cloudflare Containers、VM、Workers等の異なる実行形態を、すべて一律に「eBPF禁止」「UDP不可」と断定しません。[Cloud Runの実行契約](https://docs.cloud.google.com/run/docs/container-contract#container_security_restrictions)にある特権コンテナの制約も、そのサービスの条件であり、全クラウドを同じ条件に分類する根拠ではありません。

## 7. 記事から参照する際の境界

[確認状況](VALIDATION_STATUS.md)は公開コードの静的確認、[クラウド要約](review/2026-10-27-CLOUD-VALIDATION-SUMMARY.md)は保存済みローカル試験の要約です。今回の文書訂正は、新しいVM作成・NIC操作・再試験・実請求確認・公開siteの受入れを行ったものではありません。設計案とproviderの一般機能から本実装の対応を推定しないでください。
