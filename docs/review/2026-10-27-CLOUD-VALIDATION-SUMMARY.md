# クラウド試験の公開用要約（2026-10-27）

[公開コード・試験の確認状況](../VALIDATION_STATUS.md)と併せて参照してください。

AWS・GCPのローカル試験記録から、READMEの対応表を訂正するために要約したものです。試験対象はローカルで後続開発された単体ホスト用の `veil-local-ban` であり、公開mainの分散メッシュ全体の受入れ結果ではありません。試験対象のコード・binaryと公開mainは同一ではありません。

## AWS：2026-10-26

- Ubuntu 24.04のEC2 t3.small、x86_64、kernel 7.0.0-1014-aws、ens5／ENA、MTU 9001で一回実施。
- namespace／vethの隔離実XDPでは32件の確認が合格。BAN／解除・有限TTL・256件上限・再起動・停止復帰を含む。
- 実NICへの装着は `bpf_link_create` のEINVALで失敗。kernelログには、MTU 9001が当該guestのXDP有効時の上限3498を超える旨が記録された。3498は全ENA構成に共通する定数ではない。
- MTU・offload・装着方式を変えた再試行は実施せず、新設資源を撤去した。
- 実NICへの装着成功、装着中の管理接続維持、実NIC上のBAN／解除、公開サイトでの受入れは未達。EKSや他のNIC／MTU構成は未検証。

## GCP：2026-10-27

- Ubuntu 24.04のCompute Engine e2-small、x86_64、kernel 7.0.0-1014-gcp、ens4／VirtIO-Net、MTU 1460で一回実施。
- 修正版packageで非rootの実binary起動を確認。namespace／vethの隔離実XDPでは32件の確認が合格し、停止復帰を確認した。
- 実NICへの現行FD所有linkの作成は `bpf_link_create` のEINVAL（22）で失敗。既存loaderは `XdpFlags::empty()` と `BPF_LINK_CREATE` を使用した。
- EINVALだけでは、queue・offload・driver機能・link引数のどれが根因か確定できない。gVNICで試した結果ではなく、Genericなら成功するという証拠もない。
- mode・MTU・queue・offload・driverを変えた再試行は実施せず、新設資源を撤去した。
- 実NICへの装着成功、装着中の管理接続維持、実NIC上のBAN／解除、公開サイトでの受入れは未達。gVNIC・GKE・他構成は未検証。

## Azure・Cloudflare・Vercelの確認状況

Azure VM／AKSでの本実装の導入・XDP試験は未検証です。MANAやAccelerated Networkingを挙げた過去の構成案を、確認済みの対応として扱いません。

CloudflareのAPI遮断同期・オリジン保護と、Vercelの別Linuxバックエンド構成について、公開mainおよび手元の実装・試験文書を調べた範囲では、対応する連携実装と受入れ記録は確認できませんでした。既存環境の読取りや設計案を連携成功の証拠にせず、両者を「連携未検証」とします。これは各サービス自体の機能が利用できないという意味ではありません。

## 結論

AWS・GCPの隔離環境での成功と、試した実NIC構成への装着失敗は別の結果です。AWS・GCPがeBPFを一律に禁止しているという結論ではありません。汎用クラウド対応、複数ホストの分散防御、本番運用の成立は確認できていません。開発終了・追加課金試験終了の判断を変更するものではありません。

この要約は保存済みのローカル試験記録に基づき、新たな試験を行ったものではありません。原記録と生ログはローカルに保全しています。公開用要約には認証情報、アカウント／プロジェクト／資源ID、接続先IP、生ログを含めていません。生ログを公開していないため、この文書だけで第三者が試験結果を独立検証できるものではありません。
