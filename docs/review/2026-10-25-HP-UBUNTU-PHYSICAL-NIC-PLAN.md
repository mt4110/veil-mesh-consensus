# hp-ubuntuの物理NIC確認・自動復旧検証・LAN一件試験案

2026-10-25 JST。Sol／High、分担なし。
追記：この計画への承認後、[物理eno1の一件LAN受入れ](2026-10-25-HP-UBUNTU-PHYSICAL-NIC-ACCEPTANCE.md)を実施した。以下は承認前の計画・当時の確認範囲として保全する。

**物理eno1は読み取り確認のみで、XDP未装着。ユーザーの追加依頼により、別の匿名namespace内でGeneric XDPと操作プロセス喪失後の自動停止を実測した。次の実NIC試験は未承認・未実施。**

最終ゴールは公開サイトで安全に試用・復旧できる無償OSS。今回は、物理NICへ進むための対象・影響・復旧方法を確定する段階。
製品コード、256件上限、再起動で全BAN消失の契約、管理I/O期限は変更しない。

## 読み取りで分かったこと

| 対象 | 観測した事実 |
| --- | --- |
| host | hp-ubuntu、Ubuntu 26.04.1 LTS、Linux 7.0.0-38-generic、x86_64 |
| NIC | eno1、ifindex2、e1000e、PCI 0000:00:1f.6、MTU1500、1Gbps Full Duplex |
| IPv4入口 | 192.168.1.6/24、DHCP。default gatewayは192.168.1.1。bridge／VLAN／tunnelの下位interfaceにはなっていない |
| 管理経路 | SSH_CONNECTIONからMacの192.168.1.9 → 192.168.1.6:22を確認。戻りもeno1の同一LAN経路 |
| canary候補 | pi-nodeの192.168.1.100。pi-node側も192.168.1.6へenp0s31f6経由、source192.168.1.100のon-link経路 |
| XDP／TC | eno1のXDP・TC filterは空。qdiscは既存pfifo_fast。置換や解除の必要なし |
| host firewall | UFW inactive、nftablesはmetainfoのみ。IPv4／IPv6のfilter policyはACCEPT、IPv4 NATに追加ruleなし |
| 待受 | 全addressのSSH22、loopbackのDNS／chrony、DHCP。80／443／18443／18444の待受は観測されない |
| SSH防御 | 実効設定でpubkeyauthentication yes、passwordauthentication no、kbdinteractiveauthentication no。鍵接続とtakeのsudoは確認済み |
| IPv6 | 最新の再確認ではglobal IPv6あり、SSHも[::]:22で待受。host firewallからの制限は観測されない。外部到達性や上流IPv6 ACLは未確認 |
| 上流・公開 | routerのACL、NAT、port forward、UPnP、公開FQDN、TLS終端、CDNは未確認。hostのNATが空でも、router側の転送なしとは判定しない |

1Gbpsはリンク速度で、XDPの処理能力やサイト容量の測定値ではない。
SSH鍵認証と、Webの基礎防御・公開経路の安全性は別。hostのpacket filterは有効な防御として数えられず、公開site判定はHOLDを維持する。

途中でboot IDが`8f4bc0d8-b69b-447a-b3e5-0c2895932039`から`848e6d8d-5dd5-4083-a3b1-632569f65108`へ変化し、前回の/run配置が失われた。今回こちらからhost再起動は要求していない。
古い一時配置を前提にした準備はFileNotFoundErrorで停止し、新unitやXDPは開始していない。空の部分fixture`/run/vhr-c4ztf0az/`と失敗記録を保全した。
保存済みの同一hashのbinaryから別IDを準備し、新bootをbaselineとして追加試験を行った。NIC、address、経路、防火壁はその後に再確認している。

## nativeとGenericを区別する

実機へnetdev Generic NetlinkのGETだけで問い合わせ、eno1の`xdp_features=0`、native基本機能が広告されないことを確認した。
Linux 7.0の[e1000e実装](https://raw.githubusercontent.com/torvalds/linux/v7.0/drivers/net/ethernet/intel/e1000e/netdev.c)にもndo_bpfはない。

これだけで「XDP不可」とはしない。現在のloaderは[モード指定なし](../../veil-node/src/local_ban/kernel.rs)を使用し、[Aya 0.13.1](https://raw.githubusercontent.com/aya-rs/aya/aya-v0.13.1/aya/src/programs/xdp.rs)はLinux >=5.9でそのflagsをBPF_LINK_CREATEへ渡す。
[Linux 7.0のmode選択](https://raw.githubusercontent.com/torvalds/linux/v7.0/net/core/dev.c)は、指定なしならdriverのndo_bpfの有無でnative／Genericを選ぶ。
したがって実NIC試験は**Generic/SKBの候補**。eno1での成功・実装着modeは未検証で、nativeの性能やoffload動作の証拠にはしない。
既存programの置換・旧式netlink装着へ切り替えず、装着失敗はそのまま中止する。

## 本人のコンソール操作を前提にする前に追加検証したこと

ユーザーから「veil-arenaを使うなり独自で検証」と依頼されたため、Arenaの現checkout・引継ぎ・実行条件を調べた。
Arenaのmainは`e489def8a89c983d73ae24653b076aab97653dde`。README／flakeと別作業の未コミットfileは保全し、Arenaコードを編集していない。
既存入口は固定guestの試験で、対象製品binaryを組み込んだ新しい入力の確認が別途必要。pi-nodeでは今回Arena用Nix storeが見えず、Macの稼働VMは別用途だった。
今回はArenaの復旧・guest拡張や別用途VMへの介入を行わず、以前に受け入れたhp-ubuntuの匿名namespace方式を再利用した。**ArenaのVMで試験したという意味ではない。**

追加fixtureでは、loopbackだけの新しい親namespaceから別のserver namespaceを作り、server側loopbackに同じ製品binaryを装着した。hostのeno1へは接続しない。

| 追加試験 | 結果 |
| --- | --- |
| 成果物 | 既存binary SHA-256 e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890、object 2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f |
| 実装着 | 実kernelでGeneric XDP、mode2を確認。実行binaryと隔離namespaceを照合 |
| 通信 | 証明書・run固有bodyを検証したHTTPS正常 → TTL30秒BANで対象18443遮断 → 対照18444継続。kernel dropも増加 |
| 権限 | 非rootの一時UID。起動後の全threadでEff／Prm／Inh／Ambゼロ、NoNewPrivs=1 |
| 操作側喪失 | BANを適用した所有controller processだけをpidfdでSIGKILLし、終了-9を回収。daemon・XDP・BANはその直後も残った |
| 自動復旧 | 以後、解除・停止・daemonへのsignalは出さず観測。RuntimeMaxSec=180s／Restart=noでMainPID0、Result=timeout、NRestarts0になり、XDPが外れた |
| 時間 | systemdのActiveEnter→InactiveEnterの差は180.096441秒。controller喪失後からの観測は約177.824秒。runtime計時開始の違いを区別 |
| 通信復帰 | 自動detach後も対象・対照のHTTPSが成功 |

15確認が一回の通し実行で成功。TTL30秒は180秒より短く、BAN自体は先に自然失効する。**180秒待たないと通信が戻らない、または通信復帰の原因がdetachだけだった、という証拠ではない。**
観測したのは、操作側が消えた後に残ったGeneric XDP linkが、手動停止なしでruntime期限により解除された事実。
実際のSSH接続を切断した試験、物理e1000eの受信経路、kernel panic、OS全停止、電源断は未検証。

終了後に所有XDPと試験processの残留なし。daemon unitは意図したtimeoutを示すfailed／MainPID0、runnerはinactive／MainPID0で、どちらもtransient・非enabled。失敗表示を消して成功扱いにはしていない。
controlの親はroot:root／0700へ戻し、socket-ownerを保全。一時UIDは解放された。
新bootを基準に、既存service・account設定・host link／address／route・防火壁・EFI・BPF link一覧が前後一致。試験用秘密鍵を除いた証拠を回収済み。

## 推奨する最小の実NIC試験

**推奨は、自動停止付きの限定LAN試験。** 本体コンソールで監視し続ける運用は前提にしない。通常の操作経路断は上記の追加証拠で備えるが、物理NIC・kernel停止等の残余リスクは未解消として承認対象に残す。

| 項目 | 固定する案 |
| --- | --- |
| XDP入口 | hp-ubuntuのeno1。既存XDP／TCがあれば開始拒否。Generic/SKBの実modeとFD所有linkを確認してからBAN |
| 宛先 | 192.168.1.6:18443/TCP。一時HTTPSをこのIPv4だけへbind |
| BANする一件 | pi-nodeの192.168.1.100だけ、TTL30秒、一回。受信peerがこのIPであることをbaselineで確認 |
| 独立した正常client | Macの192.168.1.9。対象18443の正常継続と、新しいSSH22接続を確認 |
| 対照通信 | pi-nodeから同じ宛先の18444/TCP。遮断中も成功が必要 |
| BAN禁止source | 192.168.1.9とgateway192.168.1.1。宛先192.168.1.6自身も製品が自動除外 |
| 一時配置 | 新規/run/vhp-lan-c4ztf0az/。既存なら上書き・掃除せず中止 |
| daemon | vhp-lan-c4ztf0az-daemon.service、DynamicUser vhp-lan-c4ztf0az-d |
| HTTPS | vhp-lan-c4ztf0az-https.service、別のDynamicUser vhp-lan-c4ztf0az-h。Web processから管理socketへ入れない構成 |
| 停止上限 | daemonはRuntimeMaxSec180秒、HTTPSは240秒。両方Restart=no、停止猶予10秒。期限到達時に停止を開始し、終了しなければ所有processを強制終了する |
| TLS | 一日限りの試験用IP SAN証明書。private keyはhostの新規private配置とservice credentialだけ。clientはpublic certificateを一回の検証に使い、OS trust storeは変更しない |
| 公開範囲の抑制 | HTTPS unitだけIPAddressDeny=any／Allow=192.168.1.9/32・192.168.1.100/32。AF_INETのみ、application側も同じ二つのsourceを検査 |

この段階では常設account・unit、package、DNS、router、host firewall／route、NIC offload、EFI、host再起動は変更しない。試験の2 unitと/run配置、通常のjournal logだけを作る。
systemdによる[service単位のIP制限](https://www.freedesktop.org/software/systemd/man/latest/systemd.resource-control.html#IPAddressAllow=)はhost firewall全体の変更と区別する。実装置の対応は次の事前対照試験で確認し、設定値だけで有効と判定しない。

既存のRestart=on-abnormalは、LAN試験用の一時unitだけRestart=noへ変更する。runtime timeout後に再起動し続けて自動停止策を無効にしないため。製品コードと標準配布unitを変更する案ではない。

## 承認後の順序と合否

1. boot ID、eno1・address・client source、空のXDP／TC、空いている18443／18444、成果物hash、未使用の新path／unit／user名を再確認。差があれば自動的に値を置き換えず中止。
2. 一時HTTPSだけを起動。Macとpi-nodeの証明書・期待body・peer IP、新しい管理SSHを確認する。HTTP本文のIP headerだけでsourceと判定しない。
3. serverのcgroup BPF ingress／egress filterを確認し、host自身の192.168.1.6を送信元にした許可外TCP接続がtimeoutすることを対照確認。単なるHTTP403やTLS失敗では合格にしない。制限が効かなければXDP装着前に停止。
4. 空BANでeno1へ装着。実mode・所有FD link・UID／全threadのcapabilityを記録。Mac・pi-node・対照port・新規SSHがすべて正常でなければ中止。
5. 現instance／revisionのpreviewから、pi-nodeの一件だけTTL30秒で明示BAN。対象接続の失敗、drop counter増加、Mac・対照port・SSHの成功を照合。変更命令は再送しない。
6. entryがまだ有効なうちに新しいpreviewから解除し、対象HTTPSと一覧を確認。先に自然失効した場合は明示解除の合格にせず、その事実を記録して終了する。
7. daemonを停止し、所有XDP解除・対象HTTPSとSSHの復帰を確認する。その後HTTPS unitも停止し、試験portの待受なし・processなし・非enabled・既存環境の前後一致を確認。証拠は保全する。

1 entry／1 leaseの試験で、256件満杯・SIGSTOP／SIGKILL・NIC down・再起動の障害注入は物理NICで行わない。
全受信packetが追加programを通るため、scope外のSSHにも実装不具合やCPU負荷の波及があり得る。三つの正常controlを監視し、既存の低頻度probeと同じ通信期限で判定する。無影響・線速処理を事前保証しない。

## 中止・復旧

応答不明、想定外source／mode／link、管理・正常通信の失敗、Map error、所有不明を検出したら、変更命令を再送せず対象daemonを停止する。

```sh
sudo systemctl stop vhp-lan-c4ztf0az-daemon.service
```

管理経路が失われた場合も、上限180秒＋停止猶予10秒のlocal停止が働く案。今回の隔離実測はその根拠で、kernelが動かない場合までの保証ではない。
所有FDのclose後にXDPが外れたことと、新しいSSH・HTTPSの復帰を確認する。未知のprogramが残った場合、`ip link ... xdp off`で一括解除しない。
通信が戻らなければ成功扱いにせず止め、console等による診断を次の判断事項にする。host再起動・firewall flushは今回の復旧操作に含めない。
その後、今回のHTTPS unitだけを止め、今回新設するcontrolの親をroot所有0700へ戻し、一時UIDの再利用から保護する。file削除・既存fixtureの修復は行わない。

## 成果物と判定

- [対象・停止・復旧の構造化案](../../artifacts/hp-ubuntu-nic-plan-c4ztf0az/physical-trial-plan.json)
- [未適用のdaemon unit案](../../artifacts/hp-ubuntu-nic-plan-c4ztf0az/vhp-lan-c4ztf0az-daemon.service)、[HTTPS unit案](../../artifacts/hp-ubuntu-nic-plan-c4ztf0az/vhp-lan-c4ztf0az-https.service)、[未起動HTTPS helper案](../../artifacts/hp-ubuntu-nic-plan-c4ztf0az/https.py)
- [最新の読み取り結果](../../artifacts/hp-ubuntu-nic-plan-c4ztf0az/host-readonly-current.json)、[native機能GET](../../artifacts/hp-ubuntu-nic-plan-c4ztf0az/netdev-get-current.json)
- [追加の隔離復旧試験](../../artifacts/hp-ubuntu-nic-plan-c4ztf0az/deadman/acceptance-summary.json)、[全結果](../../artifacts/hp-ubuntu-nic-plan-c4ztf0az/deadman/result.json)、[独立した終了監査](../../artifacts/hp-ubuntu-nic-plan-c4ztf0az/deadman/independent-final-audit.json)

本タスクの読み取り確認・追加の隔離復旧検証・実NIC試験案は完了。**この対象と影響・復旧への承認後にのみ実NICへ装着する。**
現時点は物理NIC試験の実施前で、公開siteは引き続きHOLD。Genericでの性能、物理受信、公開経路、上流ACL、実運用の価値は別の判定に残す。
