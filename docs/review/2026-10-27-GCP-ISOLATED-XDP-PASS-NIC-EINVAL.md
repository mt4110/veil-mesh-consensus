# GCP再試験：非root実起動・隔離32確認は合格、実NIC装着はEINVAL、撤去完了

2026-10-27 JST。Sol／High、分担なし。本人が[再試験案](../local-ban/GCP_PACKAGE_RETRY_PLAN_20261027.md)の一台・最大60分・US$0.20・期限付き削除と撤去を承認し、一回実行した。

**修正版packageの非root実起動確認と、GCP内の隔離実XDPの32確認は合格。実NIC ens4／VirtIO-Net／MTU1460への装着はbpf_link_createのEINVAL（22）で失敗した。mode・MTU・queue・offload・driver・安全条件を変えず、再装着・再作成なし。09:07:22 JSTに今回新設物を撤去し、既存九区分の選択項目一致と別の不存在照会を確認した。**

最終ゴールは、小規模公開サイトへの安全な手動一時BANの限定導入。今回のマイルストーンは指定GCP構成での隔離機能・実NIC装着中の管理維持・停止解除・撤去で、**実NIC装着が未達のため不合格**。実行枠は終了。隔離合格、実NIC不合格、復旧・撤去完了を分けて残す。

## 1. 対象と時間枠

| 項目 | 実際の内容 |
| --- | --- |
| project／zone／machine | veil-x-tests／us-central1-b／e2-small、STANDARD一台 |
| image | ubuntu-os-cloud／ubuntu-2404-noble-amd64-v20261024、ID4575951068918267275 |
| VM／boot disk | vlb-gcp-r1-20261027-target、20GiB pd-standard、autoDelete=true |
| VPC／subnet／SSH rule | vlb-gcp-r1-20261027／vlb-gcp-r1-20261027-us-central1／vlb-gcp-r1-20261027-ssh |
| NIC・ネットワーク | VIRTIO_NET一個、MTU1460、IPv4-only、10.77.27.0/28、内部10.77.27.10、ephemeral外部IPv4一個 |
| 公開範囲 | 作成直前に照合したMac有線IPv4一件/32からTCP22だけ。18443／80／443・IPv6・DNS・公開証明書なし |
| 安全条件 | service accountなし、Shielded三項目有効、今回専用SSH鍵／known_hosts／VM metadata。IAM／API／課金設定変更なし |
| T0 | 08:59:44.537259 JST。最初の新設操作前に一度だけ記録 |
| 自動DELETE指定 | 09:44:44.537259 JST。VMのschedulingとboot autoDeleteを読戻し確認 |
| 全体期限／費用 | T0＋60分、運用上限US$0.20、税前計画額US$0.06 |
| 撤去・九区分照合 | 09:07:22.603265 JST。T0から458.07秒、約7分38秒 |

作成要求はRESTでrequestId付き一回、SDKの自動再送なし。作成前に九区分baseline、image、管理source、23権限等を確認した。今回もVM insertとoperationが成功したが、以前の503の根本原因を解明した意味ではない。自動DELETE設定は確認し、その時刻より前に手動撤去したため、指定時刻での実発火は試験していない。

今回の時間・通信量は計画範囲内。実請求額は未照合で、US$0.20はaccount全体への強制課金capではない。証拠退避時のNIC送受信累計は12,071,295 bytesで、80MiBの新規case停止線未満だった。新規apt導入・build・OS upgrade・鍵や権限の既存設定変更はない。Pi／hp／router／Mac network／他providerを操作していない。

## 2. 前回失敗した非root実起動は合格

archive SHA-256は2a54c4ae5ee93e45a27041e5ba5cfe1e60310f9947e36eab2126deb1326ee38f、manifest SHA-256は8852a7f835ff8673c45e7a13f990f919f62121d47d5874823d4469bd74be5e4e。承認された同じpackageを課金前に全verifyし、新規dirへrootで配置、GNU tarの明示した権限維持optionを使った。元の700／600や旧失敗archiveは保全した。

公開ホスト鍵はcontrol planeのserialから得てrun専用known_hostsに固定し、新規SSH／sudoを確認。guestはLinux7.0.0-1014-gcp、x86_64、glibc2.39、ens4／virtio_net、MTU1460、lockdownはintegrityだった。

service-checkはrootで親dir・root所有0755／0644・通常ファイル・単一link・hashを照合後、専用Transient DynamicUser unitを起動。実UID/GID64306・補助group64306、NoNewPrivs=1、全capability0、package書込み不可、6ファイルの読取り／hash照合、実行権を確認し、**実際のx86_64製品--helpがexit0**で終了した。15秒unit上限・3秒help上限を維持した。[GCPでの非root実起動結果](../../artifacts/gcp-r1-live-f6tsh90f/access.json)。

これで前回の203/EXECの配置不備を修正できたことは確認した。root実行で代替せず、以後の隔離・NIC処理もこの同じVM／packageの成功記録がある場合だけ実行するようにした。

## 3. 隔離実XDP：32確認が全て合格

hostから切り離したnamespace／vethで既存driverを一回実行。binary・object・driverの内容hashは前回と同一。runner240秒、daemon180秒、停止猶予10秒、元のTTL・安全条件を維持した。

| 確認した振る舞い | 結果 |
| --- | --- |
| 正常HTTPS、一覧空、previewは非変更 | 合格 |
| previewから一件BAN、一覧との一致、対象通信停止、kernel drop増加 | 合格 |
| 対照TCP portの正常継続 | 合格 |
| 古いrevisionの解除拒否・遮断維持、新previewによる解除・通信復帰 | 合格 |
| 保護IP・不正TTL拒否 | 合格 |
| 短期BAN中にdaemonをSIGSTOPし、カーネルで元の期限に失効 | 合格 |
| 分割掃除の上限、256件一覧、257件preview／直接操作の拒否・既存BAN非追出し | 合格 |
| 通常restartで256件消失、新instance、旧BAN／解除コマンド拒否 | 合格 |
| SIGKILLからの既存restart規則による復旧、BAN0、通信復帰 | 合格 |
| 各起動の実binary、隔離namespace、非root・全threadの権限破棄 | 合格 |
| 明示停止で所有XDP解除、HTTPS復帰、server回収、非enabled | 合格 |

resultはcomplete=true、32/32、cleanup_ok=true、server_reaped=true。runnerはMainPID0・Result=success・inactiveで正常終了した。[元driverの結果](../../artifacts/gcp-r1-live-f6tsh90f/guest-evidence/result.json)・[隔離終了と進行ゲート](../../artifacts/gcp-r1-live-f6tsh90f/isolated-final.json)。隔離HTTPSの専用試験CAによる確認で、公開証明書や外部の正常clientを検証した意味ではない。装着modeもこの記録からnative等と推定しない。

## 4. 実NIC：装着失敗の段階と、分からないこと

隔離終了を確認した後、ens4の既存XDPなし・TC ingress／egress filterなし・MTU1460・driver virtio_net・宛先10.77.27.10を再確認。保護範囲はTCP18443だけ、管理sourceを保護し、BANを追加せず一回だけ装着を要求した。Restart=no・RuntimeMaxSec180を維持した。

journalの具体的な失敗は次のとおり。

```text
Error: `bpf_link_create` failed
Caused by:
    Invalid argument (os error 22)
```

製品unitはMainPID0・failed、ens4にXDPは装着されなかった。[実NIC要求と応答](../../artifacts/gcp-r1-live-f6tsh90f/nic-start.json)・[journal／kernel／driver／queueの読取り](../../artifacts/gcp-r1-live-f6tsh90f/nic-failure-diagnostic.json)。

同じobject・製品が隔離vethで動いたことと、実NICへのFD所有link作成の拒否は別の結果。**GCPがeBPFを一律に禁止しているという結論ではない。** 現行loaderはXdpFlags::empty()でBPF_LINK_CREATEを使う。NICへの装着が成立していないため、実際にnative mode等で稼働したと記録しない。

ethtoolではvirtio_net version1.0.0、combined channel最大2／現在2を確認し、offloadも読み取り保存した。**EINVALだけでは、queue・offload・driver機能・link引数のどの条件が根因かを確定できない。** 明示Genericなら通るという証拠もない。[offloadの読取り](../../artifacts/gcp-r1-live-f6tsh90f/nic-offload-readonly.json)。queue不足やheadroom不足などを断言しない。

mode・MTU・queue・offload・driver・権限・時間制限を変える試行は行わなかった。装着中の新規SSH確認は未実施。失敗後に新規SSHが通ることは確認し、両者を混同しない。実NIC上のBAN／HTTPSは未実施で、TCP18443は公開していない。

## 5. 停止・証拠・撤去と既存保全

所有unit・Transient／FragmentPathを照合し、NIC unit停止とXDP未装着を確認。NIC helperのcleanup_ok=true、route／TC／MTUの前後一致、新規SSHを確認した。失敗unitのfailedをinactive成功へ書き換えていない。access／隔離daemon／runnerも全てMainPID0・非enabledで、隔離側のTransient unitは回収済みだった。

結果・command・journal・NIC状態等8ファイルを秘密鍵なしの許可リストで退避し、hash照合。試験TLS秘密鍵はexportしない。run専用SSH鍵はMacのprivate領域に保持した。[独立停止・保全監査](../../artifacts/gcp-r1-live-f6tsh90f/guest-evidence/guest-stop-audit.json)・[退避結果](../../artifacts/gcp-r1-live-f6tsh90f/export-result.json)。

| 今回新設したもの | 作成ID | 終了時 |
| --- | --- | --- |
| VM | 4522234213594978174 | 不在 |
| boot disk | 1002992531567433598 | 自動削除、不在 |
| VPC | 4916362729326008222 | 不在 |
| subnet | 7215265810123741072 | 不在 |
| SSH rule | 6406972700391266150 | 不在 |

作成応答ID・所有marker・creation time・依存を照合し、VM→disk不存在→SSH rule→subnet→VPC／自動routeの順で撤去。ephemeral IPv4も解放。九区分はinstances0、disks0、network1、subnet42、firewall4、予約address0、routes43、snapshot0、resource policy0に戻り、**件数と正規化した選択項目が全て開始前と一致**した。別の読取りでも今回五対象の不存在とVM／disk／予約address0を確認した。account全region／全service／全属性や確定請求額0を保証する照合ではない。

製品source・object・profile・修正版package・前回の失敗証拠等12ファイルはhash・mode・mtimeが不変。[撤去と九区分照合](../../artifacts/gcp-r1-live-f6tsh90f/cleanup-result.json)・[別の最終読取り](../../artifacts/gcp-r1-live-f6tsh90f/independent-final-read.json)・[保全結果](../../artifacts/gcp-r1-live-f6tsh90f/preservation-result.json)・[最終要約](../../artifacts/gcp-r1-live-f6tsh90f/final-summary.json)。

## 6. 採否と残タスク

- **利用可：記録したGCP guestの隔離veth範囲。** 非root実起動・実XDPの機能と復旧を確認できた。
- **不可：現行loaderと今回のens4／VirtIO-Net／MTU1460構成へ、そのまま導入すること。** 実NICへの装着を拒否された。
- **未検証：明示Genericや別のNIC構成、公開経路。** 設計・実装・新たな試験は今回に含めない。

今回の一台枠は終了し、失敗した装着の修正・再作成に残時間を流用しない。次は追加課金なしで現行FD所有linkとVirtIOの装着条件を一次資料・codeから絞り、明示Genericを含む最小の適合案を採用する価値があるか判断する段階。Generic成功を前提に実装や課金へ進まない。

残りは、①実NICへの適合方式の判断、②採用する場合だけ別承認の実NIC受入れ、③合格した場合だけ別承認の公開六条件・導入価値・撤去と運用判断。GCPに固執してproviderや設定の総当たりを続ける計画にはしない。最終ゴールは[公開六条件](../local-ban/DELIVERY_GATE.md)での限定導入判定のまま。

モデル／推論: gpt-6.1-sol／high、分担なし。入力・cache入力（入力の内数）・出力使用量は未取得。今回の製品code変更・新規buildなし。[文書検査・参照確認](../../artifacts/gcp-r1-live-f6tsh90f/document-verification.json)を保存する。

[試験履歴](../local-ban/TEST_HISTORY.md)・[四分類対応表](../local-ban/SUPPORTED_ENVIRONMENTS.md)へ戻る。
