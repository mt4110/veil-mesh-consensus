# hp-ubuntu WAN予備試験：TCP到達未確認と携帯送信元の変動

2026-10-26 JST。Sol／High、分担なし。最終ゴールは普通のブラウザが使う公開サイトで安全に試用できる無償OSS。今回の節目は、ケーブル復旧後に送信元限定のWAN経路を確認し、成立した場合だけ一件BANへ進むこと。

**結果はHOLD。LANの一時HTTPSとサービス単体の送信元制限は確認できたが、携帯回線からTCP接続できず、XDP・BANへは進んでいない。追加診断は保存直前の送信元不一致により開始せず終了した。両ホストの管理接続と試験対象の停止・復旧は確認した。**

## 復旧後の再開条件

本人がhpのLANケーブルを挿し直した後、06:31の新規SSHでeno1のUP／LOWER_UP、carrier=1、192.168.1.6/24、ssh.socket active、sudo -n true成功を確認した。kernelには06:29:15の1Gbps Full Duplexリンク成立が記録される。現在bootは05:08:28、boot ID67287546-57fc-4436-95cf-6e47df8708a8。

当日写真のNO-CARRIERと復帰記録は有線リンクの不成立・復旧を示す。引っ張られたという説明は本人の推測で、部品故障は確定していない。前日の障害時刻に該当する前bootのkernel記録は取得できず、前日の最初の切断原因まで断定しない。再起動命令は今回の作業では実行していない。

## 固定した試験条件

| 対象 | 条件 |
| --- | --- |
| 製品 | 保存済みLinux x86_64 binary。SHA-256 e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890 |
| eBPF object | SHA-256 2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f |
| host／NIC | hp-ubuntu、eno1／e1000e。192.168.1.6:18443/TCP。既存XDP／TCなし |
| LAN管理・正常対照 | Macの192.168.1.9、有線en0。PiはSSHとDockerの状態確認に使用 |
| 外部試験元 | MacのWi-Fi en1を携帯テザリングへ接続。172.20.10.2/28。各外部要求はen1を明示 |
| 公開入口 | F660Pの既存試験ルールvhp-wan-20261025aだけを、承認された外部source一件へ変更して一時on。DNS追加なし |
| サービス制限 | IPv4専用bind、IPAddressDeny=anyとLAN正常source・承認sourceだけのAllow。アプリ側もTLS前に同じsource制限 |
| 停止条件 | daemon180秒、HTTPS240秒、Restart=no、停止猶予10秒。到達・管理に異常があれば中止 |
| BAN条件 | 一件・TTL30秒、256件上限、再起動消失、instance／revisionを維持。今回はBAN段階に未到達 |

以前の/run fixtureは再起動で失われていた。新設先の不存在を確認してから排他的に作成し、転送物の全hashを照合した。旧記録を上書きせず、secret keyはhostのroot専用private領域とservice credentialだけで扱い、表示・取得していない。証明書は試験clientの明示CAとして検証し、OS trust storeや検証条件は変更していない。

## 実施と失敗段階

1. MacのWi-Fiだけを承認済みの一時自動DHCPへ変更。携帯経由の検証付きHTTPSと、有線での両SSHを確認した。
2. 新設/run fixture、試験証明書、unitを準備。unit検査は終了0。既存xfs_scrub unitのCPUAccountingに関する警告が出たが、今回のunitのエラーではない。
3. **ルーターをonにする前に**一時HTTPSだけを起動。LANでTLS1.3・HTTP200・固定run識別子・実peer一致を確認。cgroup ingress／egress BPF hookの実装着と、許可外のhost自身のsourceからTCP接続がtimeoutする対照を確認した。
4. 保存直前の本人承認後、送信元一件の転送をonにし、再読込みで既存3ルールのon／off／on維持と試験ルールonを確認。
5. controllerはLAN HTTPSの成功後、携帯側のTCP connectで失敗。約1秒のconnect期限で5回の読取りprobeを行ったが、いずれもtime_connect=0、HTTP000、curl終了28だった。TLS handshakeにもHTTPにも到達していない。
6. controllerは異常としてHTTPSを停止し、残留なしを監査。operatorは試験転送をoffへ戻して再読込み確認。**daemon起動・XDP装着・BAN／unban命令は実行していない。**

原result.jsonのcomplete=false、cleanup_ok=trueと失敗を保持した。controllerは終了1で、失敗を終了0へ隠していない。旧fixtureから継承した原probesのhostラベルpi-nodeは、このcontrollerでは携帯en1のcurlを呼ぶ分岐名であり、PiからWAN probeを発信した証拠ではない。argvとtransport情報で実経路を確認した。原ラベルを書き換えていない。

設定項目の照合には[F660Pの取扱説明書・表3-10](https://www.nuro.jp/pdf/device/manual_ZXHNF660P.pdf)も使用した。LAN側ホストはIPv4またはMAC、WAN側ホスト範囲はsourceセグメントを設定する欄として扱う。説明書で項目の意味を確認したことと、現在の回線で転送が実効することは別である。

## 携帯側の外向きIP変動と追加診断の中止

Cloudflareのtrace endpointへen1を明示した新規HTTPS要求で、携帯側の外向きIPv4が作業中に複数回変わることを実測した。試験前の許可source、失敗後のsource、追加診断準備時のsource、承認回答後のsourceは同一ではなかった。個々の値はprivate証拠に留める。

これは「一度確認したsourceを数分後の接続にも使える」という仮定を満たしていない。ただしtraceは別の宛先での観測であり、**失敗した自宅WAN向け接続の実送信元は取得できていない。IP変動がTCP失敗の唯一の原因だったとは断定しない。** ルーター／上流の到達性、宛先ごとのNAT等は未解決。

追加案は、サービス・XDPを起動せず、試験ポート宛てIPv4 SYNのheader情報だけ最大15秒・12件、非promiscuousで観測する1回の診断だった。本人から保存直前承認を得たが、直後のsource再照合が不一致になったため、合意した条件どおり保存せず終了。下書きをreloadで破棄し、既存試験ルールoffを確認した。

**追加のpacket観測、接続probe、追加転送の有効化は未実施。** 未使用の別/run fixtureと診断scriptは保全した。sourceをCIDRやanyへ広げる、全体firewallを下げる、期限を延ばす、繰り返しIP承認を取り直す方法へ移行していない。

## 終了・保全

- 試験転送offをreloadと画面で確認。既存Pi試験用on、Pi main用off、VPN用onを維持。F660P全体firewallは高、DMZ／UPnP等の変更なし。
- 二つのfixtureのdaemon／HTTPS計4unitはすべてnot-found／inactive／MainPID0。常設unit・enableなし。試験listener・process残留なし、XDP非装着。
- MacのWi-Fiを元の手入力付きDHCP・192.168.1.9へ復元。hpとPiのSSH、hp carrier=1を確認。PiのDockerはactive。
- hpのboot ID、選択serviceの状態・PID、account／sudo／SSH／GRUBのhash・metadata、netplan／systemd配置metadata、NIC、IPv4 route、host firewall、BPF link、EFIは比較一致。
- IPv6 routeのraw比較にはRAのexpires差分があった。rawは保持し、expiresだけを除いた比較は一致。addressもlease lifetimeだけを除いて一致。これを設定差分の隠蔽に使っていない。
- Piのboot ID・Dockerの状態／PID・TCP待受一覧は前後一致。ただし時間全体の無瞬断やmainnetのアプリ機能すべての正常性までは証明しない。
- host／EFI／routerの再起動、既存Pi転送変更、global firewall変更、恒久account作成、既存data削除、製品コード変更、公開DNS・site公開は実施していない。

## 次の最小判断

追記：新規サーバーを使わない[既存環境での選択肢評価](2026-10-26-EXISTING-ENV-WAN-OPTIONS.md)を作成した。固定外向きIPの環境は唯一の方法ではなく、同じTCP送信元portと接続保持で試験中のsourceを維持できるか、Pi／routerを変更せず先に調べる候補がある。キャリアNAT適合は未検証であり、この追記でWAN受入れHOLDを解除しない。

**このテザリングによる「source一件固定」のWAN受入れは保留する。** 第一候補は、既に所有・利用権限のある外部Linux等、試験中の外向きIPv4を安定して確認できる接続元。新たな購入・契約・他人のサービス利用は今回承認されていない。

次回も最初にTCP到達とhp側の実peerを確認し、そこを通過するまでBANへ進めない。ルーターの許可範囲を広げたり、VeilへIPv6・自動BAN・新しい合意方式を追加して今回の未確認を埋めたりしない。安定した外部試験元が用意できなければ公開XDP試験を保留し、既存のLAN受入れと今回の停止・復旧証拠を保持する。

## 証拠

- [物理リンク復旧](../../artifacts/hp-ubuntu-link-recovery-t0y_fewe/readonly.json)、[前bootの記録取得結果](../../artifacts/hp-ubuntu-link-recovery-t0y_fewe/previous-boot-link.json)
- [今回の計画](../../artifacts/hp-ubuntu-wan-resume-ik11ag68/plan.json)、[転送hash](../../artifacts/hp-ubuntu-wan-resume-ik11ag68/transfer.json)、[unit検査](../../artifacts/hp-ubuntu-wan-resume-ik11ag68/unit-verify.json)
- [XDP前のLAN HTTPS](../../artifacts/hp-ubuntu-wan-resume-ik11ag68/pre-wan-positive.json)、[cgroup実装着と拒否対照](../../artifacts/hp-ubuntu-wan-resume-ik11ag68/pre-wan-filter.json)
- [失敗を含む原結果](../../artifacts/hp-ubuntu-wan-resume-ik11ag68/result.json)、[失敗後のsource観測](../../artifacts/hp-ubuntu-wan-resume-ik11ag68/tether-egress-after-failure.json)
- [追加診断の保存直前不一致](../../artifacts/hp-ubuntu-wan-observed-mejpn1ge/source-before-save.json)。syn-observation.jsonは未実施なので存在しない。
- [終了状態](../../artifacts/hp-ubuntu-wan-resume-ik11ag68/final-state.json)、[保全比較raw](../../artifacts/hp-ubuntu-wan-resume-ik11ag68/preservation-comparison.json)、[RA期限差分を明記した比較](../../artifacts/hp-ubuntu-wan-resume-ik11ag68/preservation-comparison-annotated.json)

既存の[公開試験計画](2026-10-25-HP-UBUNTU-PUBLIC-SITE-PLAN.md)と[チェックリスト](../local-ban/PILOT_CHECKLIST.md)を継続使用する。本書は公開受入れの合格票ではない。
