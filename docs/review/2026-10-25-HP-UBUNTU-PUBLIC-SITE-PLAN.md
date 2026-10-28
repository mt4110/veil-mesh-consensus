# hp-ubuntu 専用試験サイトと公開一件BANの導入判断

2026-10-25 JST。Sol／Highによる読取り確認・設計。サイト実装、パッケージ導入、公開、DNS・router・host設定変更は未実施。

**最新状況（2026-10-26）:** ケーブル挿し直し後の管理到達は復旧。限定WAN試験はTCP到達未確認で停止し、携帯側sourceの短時間変動も観測した。公開受入れはHOLD。停止・保全を含む[今回の実測記録](2026-10-26-WAN-SOURCE-PREFLIGHT.md)を参照。以下の初期確認と経過は当時の記録として保持する。

最終ゴールは、普通のブラウザが使う小規模公開サイトで、安全に試用できる無償OSS。現在の節目は、実NICのLAN受入れを終えたlocal BANを公開経路で試すこと。今回は専用サイトの役割、実入口、既存防御、変更・復旧対象を整理する。公開受入れの完了とは区別する。

## 判断

**記事一覧と数ページの静的な「Veil試験ノート」をhp-ubuntuへ置く案を推奨する。CMSは作らない。**
候補名は `ban-pilot.veil-s.com`。本日のゾーンDNS一覧にこの名前はなく、既存の `api.veil-s.com` はveil-vaultのWorkerに割り当てられた別対象なので変更しない。候補名の予約・DNS登録・公開はしていない。将来の作成直前に重複を再確認する。

F660Pで通常のグローバルIPv4とNATが表示され、80／443の既存転送は観察されなかったため、直接IPv4のHTTPS公開は検討可能。ただし外部到達、送信元保持、証明書、IPv6防御は未検証。**公開試験はHOLD、設計の具体化は継続可能**と判定する。

## 今回確認した事実

| 場所 | 読取り結果 | 証明していないこと |
| --- | --- | --- |
| Cloudflare | 4つのactive zoneを表示。`veil-s.com` DNS一覧は1件で、`api.veil-s.com` → Worker `veil-vault`、プロキシ済み | 全Workers／Pagesの棚卸し、既存WAFルールの監査、既存アプリの動作確認 |
| hp-ubuntu | `eno1`、LAN IPv4 `192.168.1.6`、gateway `192.168.1.1`、グローバルIPv6あり | WANからの到達、IPv4リースの固定、将来のIP安定性 |
| host入口 | SSHはIPv4／IPv6で待受。80／443等のWeb待受なし。UFW inactive、nftablesルールなし、iptables-save／ip6tables-saveは空 | host防御が十分、IPv6 SSHが外部へ公開されている、他方式の全セキュリティ機構の不在 |
| hostサービス | nginx／Caddy／Apache／cloudflared／Dockerの各unitはnot-found、eno1のXDP非装着 | すべてのインストール済みファイルや任意の非稼働ソフトの不存在 |
| router | 本人がログインしたF660P、firmware `V1.0.10P5N9`。WAN `omci_ipv4_dhcp_1`、IPv4/v6 connected、NAT on、グローバルIPv4を表示 | ISPサービス名、上流の全経路、外部からの到達、MAP-E等の不存在を回線契約まで含めて断定 |
| router防御 | セキュリティレベル「高」、アンチハッキングoff、DMZ off、UPnP off。IPv4／IPv6の個別filter画面には無効の空の新項目フォームのみ表示 | 「高」のIPv6に対する実効動作、外部スキャンでの遮断、空フォームを有効な許可ルールと見なすこと |
| 既存転送 | Pi試験用TCP+UDP 31400–31409はon、Pi main用31410–31429はoff、VPN UDP51820はon。80／443の転送なし | 既存サービスの停止、転送先端末の全識別、既存転送の外部受入れ |

routerはステータスと既存設定を開いて読んだだけで、入力変更・保存・適用・削除・再起動は行っていない。最後はホーム表示へ戻した。秘密情報や設定バックアップ全体は取得していない。

プライベートなWANアドレス・転送先MAC等は、非公開の作業証拠 `artifacts/hp-ubuntu-public-site-plan-7vz5pj69/` 内へ限定した。Cloudflareの他サイト、写真、保存データ、秘密鍵は読んでいない。

## 試験サイトの内容

| パス案 | 内容 | 確認用途 |
| --- | --- | --- |
| `/` | 記事一覧と試験サイトである旨 | 普通のブラウザの表示・リンク移動 |
| `/notes/first/` 等 | 固定の短い記事2〜3件 | 複数のHTTPSリクエストと通常の閲覧 |
| `/assets/site.css` | ローカルの小さなCSS | 外部CDNに依存しない配信 |
| `/health.txt` | 配備ごとの固定識別子、秘密・IP・host名なし | originの期待bodyを照合。Cache-Control: no-store、ブラウザcacheを避けた新規要求 |

HTML／CSSを既存Webサーバーで配信する。投稿はローカルでファイルを編集し、承認した配備で更新する。初回は管理画面、ログイン、DB、アップロード、コメント、検索、広告、外部解析、service workerを作らない。BAN操作は既存のSSH＋ローカルUDSのままにし、Web APIへ公開しない。

Web配信はCaddyを第一候補とする。公開名の証明書取得・更新を既存機能で扱えるためで、Rust製に統一することは受入れ条件にしない。[公式Automatic HTTPS](https://caddyserver.com/docs/automatic-https)に従い、実際のversion、listen、TLS-ALPN challenge、redirect、HTTP/3、管理API、保存先、CAとの通信・同意を配備前に確認する。既定設定をそのまま実行して80／UDP443／IPv6の待受を増やさない。

「WordPressのRust版」は投稿権限、セッション、入力・アップロード検証、DB移行、backup等を持つ別の製品になる。今回必要なのは既存Web配信を壊さず一件を遮断・復帰できる証拠であり、CMSの開発はその検証を増やさない。将来記事を増やしたければ静的生成を別途検討し、今回は新規frameworkを導入しない。

## 公開経路の選択

| 経路 | 公開サイトとしての役割 | hp-ubuntuのXDPによる利用者一件BAN |
| --- | --- | --- |
| Cloudflare Pages／Workersだけで配信 | originの運用を減らせる | hpを通らないため検証にならない |
| Cloudflareプロキシ／Tunnel → hp | originを直接公開せずに配信する候補 | hp NICで元の利用者のL3送信元を見られない。Cloudflareやトンネル端を代わりにBANしない |
| 新規DNS-only名 → F660Pの限定DNAT → hp | 直接公開の管理・露出が増える | 実送信元が保持されることを外部通信で確認できれば候補 |

[CloudflareのDNS仕様](https://developers.cloudflare.com/dns/proxy-status/)ではDNS-onlyはoriginの実IPを返し、HTTP通信はCloudflareを経由しない。[CF-Connecting-IP](https://developers.cloudflare.com/fundamentals/reference/http-headers/)はHTTP headerであり、XDPが見るL3送信元を利用者IPに戻すものではない。[Tunnel](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/)はoutbound接続でoriginを接続する仕組みである。

今回は公開一件XDP試験が目的なので、**既存のプロキシを外さず、新規のDNS-only名と専用tupleを使う直接IPv4案**を第一候補にする。CloudflareのWAF／DDoS防御がその名前にも適用されるとは説明しない。共有WANへの攻撃は他のLAN利用にも影響し得る。DNS削除で一度公開したIPの情報まで回収できるわけではない。

```text
所有確認した外部IPv4ブラウザ
    → ban-pilot.veil-s.com（新規A、DNS-only候補）
    → F660P WAN TCP443（新規転送候補）
    → hp-ubuntu eno1 / 192.168.1.6 TCP443
    → host基礎防御 / 静的HTTPS配信

XDP: eno1で宛先192.168.1.6:443/TCPに対する送信元一件を扱う
管理: LANのSSH → ローカルUDS。インターネットへBAN操作を公開しない
```

これは未配備の経路案である。NAT後のtupleとL3送信元は実通信で確認する。公開IPをdaemonの`--destination`へ機械的にコピーしない。routerのhairpin経路や同一LANからの成功を外部到達の代わりにしない。

## 先に固める基礎防御

1. hpのIPv4アドレスが試験中に変わらない条件を確認する。固定化が必要ならDHCP予約等の対象を別途提示し、既存leaseを勝手に変更しない。
2. 初回のWeb待受はIPv4 TCP443の専用tupleに限定する。TLS-ALPNによる証明書検証を候補にし、TCP80の自動redirectやHTTP/3を意図せず公開しない。発行方式が成立しなければ中止して設計を見直す。DNS用の広いAPI tokenを作って迂回しない。
3. host側では管理SSHを既存LANの管理sourceに維持し、新しいWeb入口とIPv6の受信条件を明示する。必要な既存接続、DHCP、RA／ND等のICMPv6を壊す包括DROPや、UFWの無検証なenableを行わない。適用前に具体的な差分と自動・手動の戻し方をレビューする。
4. AAAAを追加しなくてもhpにはグローバルIPv6がある。公開前に未承認のIPv6入口がないか確認する。現在のVeilがIPv6を保護したとは数えない。IPv6のWeb公開は初回に含めず、必要な防御はrouter／hostで扱う。IPv6全体の無効化を試験の近道にしない。
5. routerの「高」、既存Pi／VPN転送、DMZ off、UPnP offを維持する。既存通信への効果確認なしにrouter全体の設定を変えない。hostとrouterの表示に加え、所有する外部環境から意図した入口・非公開の入口を限定的に確認する。
6. 独立した正常clientとBAN canaryの実IPv4を特定する。Macとpi-nodeは同じLANにいるので、外部から異なる送信元を得る証拠にはならない。共有NATの他利用者を巻き込まない条件を確認する。

routerの一般的な機能は[F660P取扱説明書](https://home.osakagas.co.jp/net/manual/assets/pdf/F660P.pdf)も参照できるが、配布元・firmware差があるため、その記述を今回の実効ruleと同一視しない。回線名の断定は不要で、現在のWAN表示と外部通信で実経路を確認する。

## Piの既存通信を維持する限定WAN予備試験

通常の公開導入の準備と、短時間の限定到達試験を分ける。前節のhost全体の入口整備を終えていなくても、新しい試験サービスだけへ送信元制限を置く方法は評価できる。既存のIPv6 SSH等の安全性を確認済みに変更する意味ではない。

- Macの有線Ethernet `en0` は、hp-ubuntu／pi-nodeへの管理に残す。Wi-Fi `en1` のテザリングを外部の試験出口にする。通常のブラウザや既定経路が自動的に携帯回線へ切り替わったとは仮定しない。
- 既存LAN試験と同じ192.168.1.6:18443/TCPを候補にし、IPv4だけでbindする一時HTTPSサービスを使う。systemdのIPAddressDeny=anyと明示Allowをサービス単体へ適用し、親slice・SSH・Docker・Piのunitには適用しない。対象versionでの実フィルタ装着と許可外sourceの拒否を、XDP前に再確認する。
- routerへ新設する候補は、確定した外部source限定のTCP18443→hpだけの転送1件。既存Pi／VPN転送、firewallレベル、DMZ、UPnP、DNSは維持する。画面にsource範囲の欄があったことと、制限が実効することは別なので、許可・拒否の対照を確認する。
- ルーターが1件の適用時に他のNAT接続を瞬断しないことは未検証。Piへの無影響を保証せず、重要な検証実行中の適用を避け、前後の管理疎通と確認対象サービスを監視する。許容できなければrouter変更を保留し、既存LAN証拠を使う。
- daemonはRuntimeMaxSec180秒、Restart=no、Webは240秒の候補とし、既存の有限停止条件を利用する。socket／cgroupの制限や外部接続を確認できない場合は装着しない。routerに自動失効の機能があるとは仮定せず、hostの自動停止と新設転送の無効化確認を区別する。
- BAN対象は外部試験sourceと新規試験portだけ。LAN管理sourceとgatewayは保護する。PiのWeb以外の通信を対象にしない。ただしXDPは装着したhpのNIC全体を通るので、プログラム誤動作の残余リスクは以前の実NIC試験と同様にある。
- テザリングで外部試験できても、正常clientがLAN側だけなら「単一外部source＋LAN対照」の証拠として記録する。独立した外部の正常clientや通常のブラウザtrustまで合格へ繰り上げない。
- 先行LAN試験の試験用CAを明示的に検証するHTTPS probeを再利用する場合、OSのtrust store変更・証明書検証無効化を行わない。この予備試験は、公開CA証明書を使う通常ブラウザの受入れとは別段階。

[systemdのIP制限](https://www.freedesktop.org/software/systemd/man/latest/systemd.resource-control.html)はunitのsocketに適用できるが、非対応環境では効かない場合がある。設定文字列だけで合格にしない。[実行時間上限](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html)もhost稼働を前提とし、routerの設定を自動的に戻すものではない。

2026-10-25 22:18 JSTの予備読取りでは、Macのhp／Pi宛て経路はen0、双方へのSSHは成功。pi-nodeのDockerはactiveだった。hpのdegraded表示は先行deadline試験のlo用transient unitのfailed記録であり、failed状態を消去していない。これらはPiアプリ全体の正常性やWANからの到達の証明ではない。

テザリング接続直後もWi-Fi IPv4は「手入力アドレス付きDHCP」192.168.1.9/32で、有線と同じアドレスだった。`curl --interface if!en1`による検証付きHTTPSは接続失敗。有線指定は成功。Wi-Fiをつないだことを外部試験の成功と数えなかった。

変更前の読取りを`artifacts/hp-ubuntu-tether-plan-pa5lqv34/before-readonly.json`へ保存し、本人からWi-Fiだけの自動DHCP切替・試験後の復元を承認された。`networksetup -setdhcp Wi-Fi`は終了0。22:22 JST、Wi-Fiは携帯側の172.20.10.2/28を取得し、en1を明示した検証付きHTTPSが200で成功、有線とは異なる外向きIPv4を確認した。hp／Pi宛て経路は引き続きen0で、双方の新規SSHも成功した。証拠は同directoryの`after-dhcp.json`。これは外向き接続の確認であり、WANから自宅への接続やBANの受入れではない。

WAN予備試験の下書きは同directoryの`wan-draft/`。新規候補は`/run/vhp-wan-20261025a/`と同prefixの一時HTTPS／daemon unit、routerの同名TCP18443転送1件。下書きの外部sourceと一致しない場合は許可範囲を自動拡大しない。routerは再認証が必要だったため、本人のログインと保存前の変更確認が必要。host配備・router変更・XDP装着はまだしていない。

22:27 JST、接続確認を終了し、承認どおり`networksetup -setmanualwithdhcprouter Wi-Fi 192.168.1.9`で元の方式に復元した。表示も原方式・原IPv4に一致。復元後もhp／Pi宛てはen0、両SSH成功、PiのDockerはactive。`after-restore.json`へ保存した。WAN試験再開時は携帯側の外向きIPを取り直し、下書きの古いsourceをそのまま使わない。下書きhelperの構文確認と文書検査のみ実施し、helperのroot実行・WAN受入れは未実施。

公開・root操作の承認は以前のLAN試験から流用しない。外部source、routerの新規項目、hostの新規配置、適用時間、復旧を確定した変更票を示してから実行する。

## 最短の実施順と判断ゲート

### 22:41以降の承認済みWAN試行と中止

本人の保存直前承認を受け、送信元を今回の携帯側IPv4だけに限定した`vhp-wan-20261025a`（TCP18443→192.168.1.6:18443）をF660Pへ新規保存した。再読込みで名前とon状態を確認。既存3転送はon／off／onのまま。Wi-Fiの自動DHCPによる外向き接続と、有線での両SSHは保存前に成功していた。今回の携帯IPは前回と変わっていたため、前回のsourceを流用していない。

その後、hpの保存前状態を取得するためのSSH接続がtimeoutし、中止条件として新設転送だけoffに戻した。再読込みと画面でoffを確認し、既存3転送を維持した。Wi-Fiも承認前の手入力付きDHCP・192.168.1.9へ復元した。全体firewall・DNS・既存Pi／VPN転送・経路・EFI・再起動は変更していない。

**WAN試験の機能受入れは未実施。** hpのSSH失敗は接続前で、remoteの状態取得script自体が実行されなかった。hostへの新規配備命令、試験サービス起動、XDP装着、BAN命令は一つも実行していない。hp内の最終状態は接続できず取得不能なので、データ全体の前後hash一致や復旧完了を主張しない。

off・Wi-Fi復元後も、hpのSSHはMac／Piの両方からtimeout。Piからhpへの既知IPv4／IPv6各1回のpingも応答なし。hpのARPは一度既知MACが表示され、その後INCOMPLETEだった。F660PのLANクライアント一覧にはhpの既知IP／MAC・LAN2が表示されるが、その表示だけでOS稼働を断定しない。ルーター変更との時間的な前後関係はあるものの、因果関係は未確定。スリープ・OS／NIC・LAN経路等の切り分けには、本体の画面・電源状態の確認が必要。

停止後のpi-nodeでは新規SSH、Docker active（PID2601）、従来のTCP31401–31403等の待受を確認した。ただし転送がonだった時間全体のPi外部到達や瞬断なしを証明したものではない。Piのサービス・設定変更は実行していない。

証拠・下書きは`artifacts/hp-ubuntu-wan-lq2tqvlb/`に保全。`authorization.json`、限定sourceのunit／helper／controller下書き、`abort-health.json`を含む。controllerは未実行、構文確認のみ。新設転送は**offのまま保全**し、削除していない。今回の中止を隠して再試行したり、許可source・時間条件を緩めたりしない。hpのLAN到達が戻り、原因と必要な対策を評価するまではWAN試験を再開しない。

### 10月26日の物理リンク復旧

本人の本体画面ではeno1はadministratively UP、systemd-networkdはactiveだが、NO-CARRIER／no-carrierでIPとdefault routeがなかった。その後、本人がhpのLANケーブルを挿し直し、接続が戻ったと報告した。引っ張られた可能性は本人の推測であり、コネクター等の部品故障まで確定しない。

06:31 JSTの新規SSHによる読取りで、hpのeno1はUP／LOWER_UP、carrier=1、元の192.168.1.6/24とIPv6を確認。ssh.socketはactive、sudo -n trueは終了0、XDP非装着。kernelには06:29:15のNIC Link is Up 1000 Mbps Full Duplexが記録され、ケーブル挿し直しによる復帰と整合する。Piへの新規SSHとDocker activeも確認。設定変更・再起動はこの確認では実行していない。

hpの現在のbootは05:08:28、boot ID67287546-57fc-4436-95cf-6e47df8708a8。前回とは異なるが、再起動の実行者・理由は未確認。前bootの22:00–23:00のeno1 kernel記録はNo entriesで取得できず、25日の最初の障害も同じ接触状態だったと断定しない。少なくとも26日の障害は、IP変更やSSH認証より前のcarrier未検出だった。

証拠は`artifacts/hp-ubuntu-link-recovery-t0y_fewe/readonly.json`と`previous-boot-link.json`。管理到達の復旧は確認済み。WAN試験自体の合格にはしない。再開時には再起動で失われる/run配置と現在の外部source、転送off、管理・リンクの安定を再確認する。

| 順 | 作業 | 次へ進む条件 |
| --- | --- | --- |
| 1 | 本計画をレビュー、サイトの静的ファイルと配備案を準備 | 実装・host配置の範囲を確定。まだ公開しない |
| 2 | 承認済みの範囲でLANサイト、TLS／unit／host防御を準備 | 既存SSH・Pi／VPN・IPv6の必要通信が維持され、rollback可能 |
| 3 | 確定WAN・FQDN・転送・外部clientを再確認し変更票を承認 | DNS-onlyのIP露出、必要なTLS通信、有限の公開時間を理解・承認 |
| 4 | 基礎Webだけ公開し、外部baselineと送信元保持を確認 | 正しい証明書・期待body、IPv4の実source／tuple、意図しないIPv6入口なし |
| 5 | 既存の停止deadlineを持つdaemonを空BANで装着 | 正常client、新規SSH、対象外通信が継続 |
| 6 | 所有canary一件のTTL30秒BAN、解除・自然失効・通常daemon再起動 | 対象だけ遮断。新instance・BAN0。古い操作拒否。時間条件の変更なし |
| 7 | 新設試験対象を停止・非enabled、公開経路の新規差分を戻す | XDP非装着、正常通信、既存データ・他サービス維持、証拠保存 |

local BANの256件上限、満杯時拒否、RAMのみ・再起動でBANが消える契約、instance／revision、結果不明の自動再送禁止は維持する。満杯・破損・SIGKILL等の障害注入を公開サイトで再実施しない。ホスト再起動は含めない。

公開試験の詳細な操作と停止条件は、既存の[チェックリスト](../local-ban/PILOT_CHECKLIST.md)、[操作手順](../local-ban/OPERATIONS.md)、[復旧手順](../local-ban/RECOVERY.md)を使う。[物理NICのLAN受入れ](2026-10-25-HP-UBUNTU-PHYSICAL-NIC-ACCEPTANCE.md)を公開受入れの代用にしない。

## 次の変更票で示す対象・影響・復旧

以下は承認依頼の準備項目であり、今回の変更承認でも実行可能な完成設定でもない。

| 対象候補 | 影響・承認前に確定すること | 復旧の準備 |
| --- | --- | --- |
| hp専用サイト配置／Web unit | 新規ファイル・専用UID・TLS保存先・packageと自動起動有無。既存unitやdataを上書きしない | 今回のWeb unitだけ停止・非enabled。新規配置を保全し、所有を確認して退避 |
| hpのhost入口制限 | IPv4／IPv6ともSSH・DHCP・ICMPv6等を維持する具体的rule差分。管理遮断リスクを評価 | 新規ruleだけを識別して戻す。全flush・全体resetを使わない。別の停止deadlineも準備 |
| F660Pへの新規TCP443転送 | WAN→hpのexact宛先、TLSの要求、影響時間。共有回線への露出 | 新設項目だけを無効化して閉鎖確認。既存3転送と全体firewallは変更しない |
| Cloudflareの新規A | 候補名・確定WAN IP・DNS-only・TTL、IP公開。既存Workerは維持 | 新規名だけの無効化／削除の扱いを明示承認。DNS cache中もrouter／hostで閉じる |
| local BAN一時unit | 確定tuple、保護source、既存binary hash、有限deadline、Restart=no | 所有unit停止→XDP解除→実通信復帰。未知programの強制detachなし |

原則は二段階に分ける。LAN配置・基礎防御を先にレビュー可能にし、その結果と確定値を使って公開変更票を作る。Webコードを増やしてWAN／IPv6の未確認を埋めたことにはしない。

## 限界と継続判断

- 一件BANの成功は自動検知、攻撃識別、大規模DDoS耐性、回線帯域の保護を証明しない。
- IPv4の同一送信元NAT配下は同じBAN対象になり得る。IP変更・別出口は回避経路になる。
- 同じ宛先IP／portの別hostname・pathをXDPで分けない。初回は専用tupleにする。
- 普段の公開がCloudflare経由だけなら、利用者の制御もCloudflare／HTTP層で行う方が自然。XDPを採用するためだけに既存CDNを外す必要はない。
- 直接公開の管理負担に見合う用途がなければ、公開XDP試験を保留し、LANの実証を到達点にしてよい。今回の小サイトはVeilの技術受入れに使い、別CMS製品へ拡張しない。

## 証拠と未実施

新規private証拠: `artifacts/hp-ubuntu-public-site-plan-7vz5pj69/host-readonly-inventory.json`、`legacy-filter-readonly.json`、`browser-readonly-findings.json`。
既存の失敗・実験記録とBFT実装は保全。今回は文書の追加と読取り証拠の保存だけで、実装回帰や公開通信試験はしていない。

残る実確認は、IPv4外部到達とsource保持、IPv6の実効入口制限、信頼できるTLS、外部の独立canary／正常client、試験後の閉鎖・復帰である。これらが揃うまでは「公開サイトに導入済み」と判定しない。
