# 現行XDP一時BANの対応環境と確認範囲

更新: 2026-10-27 JST。対象は現行の`veil-local-ban`と検証済みXDP object。同じ期限契約での標準nftables移植は[不採用](../review/2026-10-27-NFT-BOOTTIME-ADOPTION-DECISION.md)のまま、再開していない。

**全てのserverで使えるとは判定できない。Linuxの権限・kernel・実NICと装着mode・入口が一致する構成に限って評価する。** 最初は保存済み証拠・既存code・公式仕様・GCP読取りで整理した。後続の明示承認で[一台試験](../review/2026-10-27-GCP-VM-BOOT-AND-EXEC-PERMISSION-FAILURE.md)を実行し、VM・SSH成功と試験配置不備、撤去を追記した。製品code・期限・安全条件は変更していない。

最終ゴールは、小規模公開サイトで安全に使える手動一時BAN。現在のマイルストーンは導入可能な構成の特定。最新の作業は[明示Genericとoffload保全guardの実装・非課金検証](../review/2026-10-27-GENERIC-OFFLOAD-GUARD-IMPLEMENTATION.md)まで。新loaderの実NIC・GCP受入れは未検証で、過去の実測と分ける。

後続の[GCP専用一台の具体案](GCP_GENERIC_SINGLE_VM_PLAN_20261027.md)は未承認・未実行。非課金で新x86_64版・package／helperの適合を先に確認し、offload事前設定と復元を含む最大60分・US$0.20の実行は確定値の別承認後とする。計画作成時の読取り成功を装着成功へ読み替えない。

## 1. 四分類の意味

| 分類 | 意味 |
| --- | --- |
| 利用可 | 記載した同じ環境・成果物・経路の範囲で、動作と終了復旧の証拠がある。公開運用や他NICへ一般化しない |
| 条件付き | codeと公式仕様上の必要条件を満たす場合の候補。対象ごとの実測合格を意味しない |
| 不可 | 現行実装・権限・入口では目的を満たせない、または同じ構成で装着失敗を実測した。環境全体の永久的な不可能とは区別する |
| 未検証 | 該当構成で必要な実測がない。公式上の候補でも成功扱いにしない |

## 2. 対応表

| 環境・用途 | 判定 | 根拠・限界 |
| --- | --- | --- |
| hp-ubuntu、Ubuntu 26.04.1／7.0.0-38、eno1／e1000e、MTU1500、Generic XDP | 利用可：確認済みLAN一件の範囲 | 対象HTTPSの遮断・期限前解除、正常client／新規SSH／対照port、停止復帰を[実確認](../review/2026-10-25-HP-UBUNTU-PHYSICAL-NIC-ACCEPTANCE.md)。公開六条件は未達 |
| pi-nodeの隔離namespace／veth／実XDP | 利用可：保存した隔離受入れ範囲 | 一覧・preview・適用・解除・期限・256件・restartを[確認](../review/2026-10-25-LOCAL-BAN-OPERATOR-KERNEL-ACCEPTANCE.md)。Piの物理入口や公開経路の合格へ転記しない |
| AWS t3.small／Ubuntu 24.04／7.0.0-1014-aws、隔離veth | 利用可：当時の隔離範囲 | 同一hashの製品による[32確認](../review/2026-10-26-AWS-SINGLE-VM-XDP-AND-MTU.md)。VMは撤去済み |
| 同AWS構成のens5／ENA／MTU9001 | 不可：現構成のまま | 実NICへの装着をkernelがMTU条件で拒否。eBPF一律禁止という結果ではない |
| ENAをMTU1500にした別構成 | 未検証 | 改善候補であって成功証拠はない。今回変更・再試験しない |
| GCP e2-small／固定Ubuntu 24.04／7.0.0-1014-gcp、隔離veth | 利用可：記録した隔離範囲 | 非root実--help、実XDPの32確認、期限・256件・restart・停止復帰を[再試験で確認](../review/2026-10-27-GCP-ISOLATED-XDP-PASS-NIC-EINVAL.md)。VM撤去済み。公開経路へ一般化しない |
| 同GCP guestのens4／VirtIO-Net／MTU1460、現行loader | 不可：この構成のまま | bpf_link_createがEINVAL（22）で拒否。拒否条件の内訳は未確定。mode・MTU等を変えず撤去 |
| GCPで保存したGRO_HW:onのまま、一行だけGenericへ変更 | 不可：設定保全条件下では不採用 | kernelのGeneric装着はoffloadを変更し得て、link解放で復元しない。[採否根拠](../review/2026-10-27-GCP-XDP-ATTACH-DESIGN-DECISION.md) |
| 事前にLRO／GRO_HWのactive／wantedがoffの専用環境＋明示Generic／保全guard | 条件付き：実装・単体検証まで | 明示mode・GET・拒否条件・READY前確認を実装。Linux ARM64の非root回帰に合格。新loaderの実XDP、実NICでの装着・feature保全・GCPは未検証。共用環境や不明なfeatureは拒否 |
| GCP gVNIC／gveの対応queue形式・driver・設定 | 条件付き | Google公式driverにXDP対応と設定条件がある。今回のloaderでの実装着は未検証で、別機種を自動追加しない |
| 自分で管理できる他のLinux VM／物理server | 条件付き | 下記の権限・kernel・NIC・入口の条件を満たし、対象の隔離／実NIC受入れが必要。rootがあれば必ず動くとはしない |
| Cloud Runのcontainer内へ現行XDPを直接配備 | 不可 | 公式runtime contractがeBPFとhost権限操作を非対応としている |
| 管理権限のない共有hosting・通常の非特権container | 不可：権限のない現構成 | host／対象namespaceのXDPをload・attachできない。privileged化やhost権限付与を解決策として自動実行しない |
| Windows／macOSへ現行Linux binaryを直接配備 | 不可：現行実装 | Linux BPF／XDP／BOOTTIME経路を要求する。Linux VMを別に作った場合は別の評価対象 |
| 既存XDPが対象NICを使用している環境 | 不可：同じNICへそのまま併設 | 現行loaderは既存programを置換しない。CNIやdispatcherとの共存機構は未実装 |
| CDN／別hostのL7 proxyの裏で、元の利用者IPごとにXDP BAN | 不可：その用途 | XDPが照合するIPは実際のpacket source。HTTP headerで転送された元利用者IPには対応しない。中継sourceのBANで広く巻き込む運用は採用しない |
| IPv6／UDP／HTTP3等を含むsite全体の防御 | 不可：現行機能の対象外 | 本モードは指定IPv4／TCP入口だけ。対象外trafficはPASSする。server上でbinaryを起動できることと、全経路を保護できることは別 |

Cloud Runは[公式のsecurity restrictions](https://docs.cloud.google.com/run/docs/container-contract#container_security_restrictions)、gVNICは[Google公式driverのXDP条件](https://github.com/GoogleCloudPlatform/compute-virtual-ethernet-linux#xdp)を参照。CDNの元利用者情報は[Cloudflare公式のCF-Connecting-IP説明](https://developers.cloudflare.com/fundamentals/reference/http-headers/#cf-connecting-ip)と、既存Cのpacket header処理を照合した。HTTP headerの情報をL3 sourceと同一視しない。

## 3. 変えない必要条件

1. Linux >=5.9と、現行objectのhelper／map／BPF_XDP FD所有linkを利用できるkernel・起動権限。起動後は既存profileどおり非root・capabilityなしを確認する。
2. CPU architectureとlibcに合う製品binary。今回再利用するのはx86_64で確認した同一hashのbinary。別architectureへそのまま配布しない。
3. 対象NICのdriver・MTU・queue／buffer等の条件と、実際の装着mode。既存XDP／TCを事前に確認し、未知programの置換・削除や設定変更で通さない。
4. 実sourceが確認できる一つの宛先IPv4・TCP port。管理sourceを保護し、実際にpacketがそのNICのXDPを通ること。
5. Rustで決めたCLOCK_BOOTTIME絶対期限、有限TTL、256件上限・満杯時拒否、古いinstance／revision拒否、restartでBAN0、停止で所有link解放を維持する。

[kernel.rs](../../veil-node/src/local_ban/kernel.rs)の新sourceはdefaultで`XdpFlags::empty()`、genericで`SKB_MODE`をAya 0.13.1へ渡す。Linux >=5.9のBPF_LINK_CREATE一回だけを使用し、FdLinkを保持する。**明示Genericは実装したが、自動fallbackはない。** mode・program ID・feature保全をREADY前に読取り確認する。実機受入れは別途必要。

**Genericの追加制約:** [一次資料調査](../review/2026-10-27-GCP-XDP-ATTACH-DESIGN-DECISION.md)で、Generic装着がLRO／hardware GROを無効化する処理を呼び、解除だけでは設定を復元しないと確認した。新guardはdefaultも含む全attach経路で両featureのactive／wanted事前offを必須とする。HPの旧LAN合格は維持するが、旧証拠だけでoffload保全や新loaderを確認したとはしない。GETと装着の間の他管理者の競合、任意native driverの設定不変、未知の全TC hookへの対応は保証しない。[導入条件と互換性変更](INSTALL.md)を参照。

複雑なreverse proxy、DNAT、load balancer、CNI、offload、IPv6を含む新しい構成は、既存の単純な経路の証拠だけで利用可へ上げない。公開運用での性能・回線飽和耐性・無人防御の価値も未証明。

## 4. 作成前のGCP読取り結果（保存した準備記録）

projectを`veil-x-tests`と明示し、既存CLI認証で15件のlist／describe等を実施、全てexit0。default projectの変更、API有効化、IAM変更、作成要求は行っていない。[privateなコマンド記録](../../artifacts/gcp-xdp-feasibility-xcc4vbyo/read-commands.json)・[要約](../../artifacts/gcp-xdp-feasibility-xcc4vbyo/read-summary.json)。

| 読取り項目 | 結果 |
| --- | --- |
| project／Compute／billing | ACTIVE、Compute API有効、billingEnabled=true |
| 現在の対象project | instance0・disk0・予約address0。network1・subnet42・firewall4・route43は保全対象 |
| zone／machine | us-central1-bはUP、e2-smallは2 vCPU・2048MiB。実hostのCPUや収容能力を保証しない |
| image | ubuntu-os-cloudのubuntu-2404-noble-amd64-v20261024、ID4575951068918267275、READY／X86_64、deprecatedなし |
| quota | us-central1でE2_CPUSはlimit24／usage0、instance24／0、IN_USE_ADDRESSES8／0。quota増加は申請しない |
| 新subnet案 | 10.77.27.0/28は取得した既存subnetと重複なし |
| 管理元 | Macの有線en0を確認し、その経路の外向きIPv4を取得。正確な/32はprivate planに保存し、作成前に再確認。携帯回線は使わない |

これは読み取りAPIの成功で、**過去のVM作成HTTP503の解消、支払い手段の全状態、作成権限、実kernel／NIC対応を証明しない。** 過去の[一台作成失敗](../review/2026-10-26-GCP-SINGLE-VM-CREATE-503.md)はguest作成前であり、XDP拒否の結果へ読み替えない。今回の作成要求は0件。

## 5. 再試験後の判断と終了条件

前回の[配置権限の失敗](../review/2026-10-27-GCP-VM-BOOT-AND-EXEC-PERMISSION-FAILURE.md)を追加課金なしで修正し、別承認で[一台再試験を実行](../review/2026-10-27-GCP-ISOLATED-XDP-PASS-NIC-EINVAL.md)した。非root実起動・隔離実XDPの32確認は合格。一方、実ens4はFD所有link作成をEINVALで拒否し、実NIC装着中のSSHやBANは未検証。09:07:22 JSTに新設物撤去と既存九区分の選択項目一致、別照会の不存在を確認した。

GCPのeBPF一律禁止とはしないが、保存した実NIC構成へそのまま導入する案は不可。今回枠は終了。[採否判断](../review/2026-10-27-GCP-XDP-ATTACH-DESIGN-DECISION.md)後、本人が事前offの専用環境案を採用し、guardと明示Genericの実装・非課金検証まで進めた。**保存したrx-gro-hw:on状態では新guardも拒否する。** NIC事前設定の対象・影響・復旧方法と新しい実NIC試験の費用・期限・撤去を確認するまで、設定変更・GCP再試験は行わない。実NIC合格後だけ公開六条件へ進む。環境を用意できなければGCP経路は保留する。

モデル／推論: gpt-6.1-sol／high、分担なし。入力・cache入力（入力の内数）・出力使用量は未取得。製品code・既存host／networkへの変更なし。
