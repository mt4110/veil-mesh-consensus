# hp-ubuntu物理eno1の一件LAN受入れ

2026-10-25 JST。Sol／High、分担なし。
**承認された一件LAN試験を、物理eno1へGeneric XDPを装着して実施した。対象HTTPSだけの遮断、正常client・新規SSH・対照portの継続、有効期限前の明示解除、XDP停止後の復帰を確認。終了監査の自己観測による誤検出は原記録を残して独立監査で切り分け、終了・保全も確認した。**

最終ゴールは公開サイトで安全に試用・復旧できる無償OSS。今回のゴールは、[承認した計画](2026-10-25-HP-UBUNTU-PHYSICAL-NIC-PLAN.md)の一件試験を実NICで受け入れること。
今回の範囲はLANの一時HTTPS。公開site・常設配備・本人のブラウザ操作の合格ではない。

## 実行対象と承認範囲

| 対象 | 実測・使用したもの |
| --- | --- |
| host | hp-ubuntu、Ubuntu 26.04.1 LTS、Linux 7.0.0-38-generic、x86_64 |
| NIC | eno1、e1000e、ifindex2、MTU1500。開始前にXDP／TCなし、試験portの待受なしを確認 |
| 宛先・遮断port | 192.168.1.6:18443/TCP |
| BAN source | pi-nodeの192.168.1.100、一件、一回、TTL30秒 |
| 正常・管理source | Macの192.168.1.9。gateway192.168.1.1とともにBAN禁止。宛先自身も自動除外 |
| 対照 | pi-nodeから192.168.1.6:18444/TCP |
| binary SHA-256 | e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890 |
| C object SHA-256 | 2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f |
| 一時配置 | /run/vhp-lan-c4ztf0az/。新規専用path、上書きなし |
| daemon unit | vhp-lan-c4ztf0az-daemon.service。DynamicUser、RuntimeMaxSec180秒、Restart=no、停止猶予10秒 |
| HTTPS unit | vhp-lan-c4ztf0az-https.service。別DynamicUser、RuntimeMaxSec240秒、Restart=no、IPv4限定 |

両unitはsystemd-runによるtransientで、常設unit file・enable・account DB追加なし。試験前後の73fileのsource hashと、終了後の配置binary／object／helper／unit案のhashも一致した。
製品コード、256件上限、再起動消失、TTL規則、管理I/O期限は変更していない。host再起動、EFI、host firewall・経路・offloadの設定変更も行っていない。

## XDP装着前の送信元制限

HTTPSは192.168.1.6の18443／18444だけにbindした。試験証明書のIP SANを検証し、run固有bodyと受信peer IPを照合した。
private keyはhostの新規root専用pathとservice credentialで使用し、内容を表示・回収していない。clientのOS trust storeも変更していない。

- Mac／pi-nodeの両方から、証明書・body・期待peer IPが一致するHTTPSが成功。
- HTTPS service自身のcgroup ingress／egress BPF filterを確認。
- 許可外のhost自身の192.168.1.6をsourceにしたTCP接続は、HTTP／TLSより前のconnectでtimeout。両許可sourceの成功も再確認してからXDPへ進んだ。
- IPAddressDeny=any、Allowは192.168.1.9/32と192.168.1.100/32。application側にも同じsource制限を置いた。host全体の防火壁へruleを追加したのではない。

## 実NICで確認できたこと

| 確認 | 結果 |
| --- | --- |
| 実装着 | eno1のGeneric XDP、mode2。program ID97、XDP link ID4、ifindex2を確認し、そのlink FDをdaemon自身が保持していることを/procのfdinfoでも照合 |
| 実体・権限 | 実行中binaryのhash一致、hostのnet namespaceであることを確認。daemon UID64735、起動後の全threadで非root・Eff／Prm／Inh／Ambゼロ・NoNewPrivs=1 |
| 空BAN | capacity256、entry0。Mac・pi-nodeのHTTPS、対照18444、新規SSHがすべて成功 |
| 保護source | Macとgatewayへのpreview-banが拒否され、実行commandを出力しない |
| 一件BAN | 現instance／revisionのpreviewを確認し、pi-nodeだけへTTL30秒で明示適用。一覧は一件・active／confirmed |
| 対象遮断 | pi-nodeの新規対象TCP接続がconnect段階でtimeoutし、kernel drop counterも増加。curl失敗やTLS失敗だけを根拠にしていない |
| 対象外継続 | BAN中もMacの対象HTTPS、pi-nodeの18444、新しい管理SSH接続が成功 |
| 明示解除 | 新しいpreviewのunbanを一回実行。解除完了は期限より約26.069秒前で、自然失効を解除成功に取り違えていない |
| 解除後 | 一覧0件。pi-node・MacのHTTPS、対照port、SSHが成功 |
| 停止後 | 所有daemon停止後にeno1のXDP解除を確認し、HTTPSと新規SSHの復帰を確認。その後一時HTTPSも停止 |

機能確認23項目が成功。変更命令はBAN一回・unban一回だけで、再送・自動token更新・追加leaseはない。
BAN応答からunban応答まで約3.930秒。これはprobeやSSH・状態確認を含む試験操作の経過時間で、製品の単独処理レイテンシや性能benchmarkではない。
新規SSHは計5回成功。稼働中の既存SSHだけを管理経路の証明にしていない。

256件満杯、自然失効、normal／SIGKILL restart等は[先行の隔離受入れ](2026-10-25-HP-UBUNTU-ISOLATED-XDP-ACCEPTANCE.md)の証拠。今回の物理NICではその障害注入や反復BANをしていない。
操作側喪失と180秒のGeneric XDP自動detachは[追加の隔離復旧試験](2026-10-25-HP-UBUNTU-PHYSICAL-NIC-PLAN.md)で確認したもので、今回物理NICのSSHを意図的に遮断した試験とは区別する。

## 監査の不具合と独立した終了確認

一回の試験driverはfunctional_complete=trueだったが、cleanup_ok=false／complete=falseを出した。CLI process終了は0だったため、終了コードだけで合格にしていない。
原因はfinal監査のprefix照合が、自分を起動して待っているsudo親processを残留として数えたこと。PID6474のjournalには、まさに今回のremote.py finalの起動とsession終了が記録されていた。
原result.json・実行log・誤検出を保持し、値を書き換えていない。

別接続で、prefixをargvに含めない独立監査を実行し、PID6474は既に存在せず、試験processなし・待受なし・XDPなしを確認した。
さらに別の監査helperで、実際に自分とsudo親がprefix候補へ入る条件を再現した。

1. 現observerと、その同じ監査scriptを呼び出す実ancestorだけを区別し、残留0を確認。
2. prefixを持つ所有する無動作processを一つ起動し、それは実際の残留として検出。
3. そのprocessを通常終了・回収し、再び残留0を確認。

この対照回帰は終了0。監査自身を除外しても、別の本当の残留を隠さないことを確認した。製品やXDPの試験を再実行して帳尻を合わせたわけではない。
集約の合格判定は、機能確認と独立終了監査の両方に基づく。raw driverの誤判定と、将来再利用時に監査・終了コード処理の修正が必要な点を明記する。

最終状態：

- daemon／HTTPSともnot-found、inactive、MainPID0。常設fileと自動起動symlinkなし。
- eno1のXDP／TCなし。18443／18444の待受なし。試験process残留なし。
- 一時user二つは解放。controlはroot:root／0700で、残るentryはsocket-ownerのみ。未知のfileや既存dataを削除・修復していない。
- boot ID、既存serviceの状態・PID・unit path、account／sudo／SSH／GRUB設定、/etc/netplanと/etc/systemd/systemのmetadataが前後一致。
- host link、IPv4／IPv6 address・route、防火壁、BPF link一覧、EFIも前後一致。IPv6の期限等だけ比較時に正規化し、rawは保持。
- offload設定は、前回の計画時snapshotと今回終了後が一致。直前に別途採ったsnapshot同士と誤記しない。

最後に一時HTTPSを停止したため、試験portは閉じている。XDP解除後に通信復帰を確認したうえでの意図した終了である。
対象を絞った保全比較であり、host全dataの完全整合性や、任意の第三者介入がないことまで保証する監査ではない。

## 証拠と残る段階

- [集約と独立保全判定](../../artifacts/hp-ubuntu-lan-csld2uup/acceptance-summary.json)
- [原driver結果・全RPC／probe](../../artifacts/hp-ubuntu-lan-csld2uup/result.json)、[実行log](../../artifacts/hp-ubuntu-lan-csld2uup/execution.log)
- [独立終了監査とPID6474のjournal](../../artifacts/hp-ubuntu-lan-csld2uup/independent-final-audit.json)
- [自己観測誤検出と実残留検出の対照回帰](../../artifacts/hp-ubuntu-lan-csld2uup/audit-regression.json)、[監査helper](../../artifacts/hp-ubuntu-lan-csld2uup/audit-check.py)
- [開始前](../../artifacts/hp-ubuntu-lan-csld2uup/host-before.json)／[終了後](../../artifacts/hp-ubuntu-lan-csld2uup/host-after.json)、[成果物hashとoffload](../../artifacts/hp-ubuntu-lan-csld2uup/final-artifacts-offloads.json)
- remoteの新規配置は/run/vhp-lan-c4ztf0az/に保持。次のhost再起動で失われる。秘密鍵を除いた必要な証拠はlocalに取得済み。

**今回の物理eno1・一件LAN受入れは完了。公開siteはHOLDを維持する。**
次は専用site、公開URL、router／NAT／上流IPv6 ACL、実送信元、HTTP側の基礎防御、公開時間と復旧経路を確定し、公開canaryへ進む承認を得る。
ブラウザの通常trust、公開経路、別NIC／native mode、IPv6／HTTP3、防御の性能・有効な運用負荷は未検証。今回の成功をそれらの合格に転記しない。
