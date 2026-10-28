# GCP一台の再試験案：配置属性と非root起動を先に確認

2026-10-27 JST。Sol／High。以下は承認前に作成した計画を保全したもの。**後続の本人承認で一回実行し、非root実起動と隔離32確認は合格、実NIC装着はEINVALで失敗した。mode等を変更せず約7分38秒で撤去し、既存九区分一致と別照会の不存在を確認。今回枠は終了し、追加試験への許可ではない。** [実結果と残タスク](../review/2026-10-27-GCP-ISOLATED-XDP-PASS-NIC-EINVAL.md)。 [前回計画](GCP_XDP_SINGLE_VM_PLAN_20261027.md)と[前回の失敗・撤去](../review/2026-10-27-GCP-VM-BOOT-AND-EXEC-PERMISSION-FAILURE.md)を保全する。

## ゴールと、この試験を続ける理由

最終ゴールは、普通のブラウザで使う小規模公開サイトについて、運用者が既知の送信元IPv4一件を指定TCP入口で有限時間BANし、解除・元の期限での失効・restartでBAN0・停止復帰を安全に行える無償OSSを、限定試験導入できること。[公開六条件](DELIVERY_GATE.md)を維持する。

GCPは導入先候補の一つ。今回の次のマイルストーンは、**同一製品の隔離機能と、指定した実仮想NICでの装着・新規SSH維持・解除・撤去**を一回で判定すること。対応hostや試験回数を増やすことを完成条件にしない。BFT再開、標準nftables移植、autoBAN、独自暗号、他provider・大規模対応へ広げない。

前回は試験packageの権限不備で製品のexec前に停止した。修正対象は内容・期限・権限分離ではなく、その配置と起動前確認。[今回の追加課金なしの検証](../review/2026-10-27-GCP-PACKAGE-PREFLIGHT-FIX.md)を確認してから、以下の新しい枠を判断する。

## 固定する新設対象と影響

| 項目 | 新規承認を求める案 |
| --- | --- |
| project／zone／machine | veil-x-tests／us-central1-b／e2-small、STANDARD一台 |
| image | ubuntu-os-cloud／ubuntu-2404-noble-amd64-v20261024、ID4575951068918267275。作成前再照合 |
| VM／boot disk | vlb-gcp-r1-20261027-target、20GiB pd-standard、boot autoDelete=true |
| VPC／subnet／SSH rule | 新規vlb-gcp-r1-20261027／vlb-gcp-r1-20261027-us-central1／vlb-gcp-r1-20261027-ssh。custom・MTU1460・10.77.27.0/28・IPv4-only |
| NIC | 一つのVIRTIO_NET、内部10.77.27.10、ephemeral外部IPv4一つ |
| 公開範囲 | 作成直前に照合するMac有線source一件/32→TCP22だけ。直近の承認値111.217.200.148/32から変化していれば保存せず確認。18443・80・443・IPv6は公開しない |
| 身元・保護 | SA／scopesなし、Shielded三項目維持、新規runだけのSSH鍵・known_hosts・VM metadata。OS Login／組織policyを無効化しない |
| guest新設先 | 新VM内だけの/run/veil-gcp-xdp-20261027/。旧VMは撤去済みで、ローカル証拠・既存hostのfixtureは上書きしない |
| 追加の事前確認unit | veil-gcp-xdp-20261027-access.service。Transient・DynamicUser、capabilityなし、PrivateNetwork、Restart=no、RuntimeMaxSec15秒、停止猶予10秒 |
| 製品・HTTPSの時間 | 実NIC daemon180秒・Restart=no・停止猶予10秒。隔離HTTPS／runner240秒。隔離driverの既存restart caseは維持 |

Pi・hp・router・Mac network・共有SSH／CLI設定・既存GCP resource・IAM／API／課金設定は変更しない。新規key／unit／packageは新VMと新private run dirだけ。DNS・公開証明書・二台目・LB・NAT gateway・監視agent・Cloud Scheduler等は追加しない。

## 変更したpackageと、飛ばせない確認

[package helper](../../scripts/local-ban-trial-package.py)が新しいUSTAR entryでroot所有・binary0755・object等0644を固定する。ACL／xattr／PAX、symlink／hardlink、余分なpath、重複member、不明なhash等を拒否する。元binary0700／object0600や旧archiveは変更しない。control dir・秘密鍵はpackageに含めない。

使用候補はprivateな[修正版package](../../artifacts/gcp-package-fix-ku098if1/payload-v2.tar.gz)。archive SHA-256は2a54c4ae5ee93e45a27041e5ba5cfe1e60310f9947e36eab2126deb1326ee38f、manifest SHA-256は8852a7f835ff8673c45e7a13f990f919f62121d47d5874823d4469bd74be5e4e。製品・object・既存driverの内容hashは前回と同一。[詳細manifest](../../artifacts/gcp-package-fix-ku098if1/package-v2-result.json)。helperやpackageを変更した場合はhashを更新して再確認し、古い確認を流用しない。

1. 課金前にMacでpackageの全member・mode・uid/gid・hashを再検証する。
2. 承認後の新VMだけで、未使用のguest dirへrootで配置する。検証済みarchiveをGNU tarの`--same-permissions --no-same-owner --no-acls --no-xattrs`で展開し、umaskによる0700への再変換を避ける。未検証archiveへこの操作を使わない。
3. [access helper](../../scripts/local-ban-trial-access.py)のservice-checkをrootで呼ぶ。親dir・通常ファイル・単一link・実uid/gid／mode・manifest／hashを確認してから、上表のcapabilityなしDynamicUser unitでuser-checkを起動する。
4. 非rootの読取り・実行権・書込み不能・NoNewPrivs・全capability0に加え、**実際のx86_64製品の--helpが3秒以内・exit0**を必須とする。ここではserve・socket接続・BPF loadを行わない。
5. service-checkのexit0とJSON内のnative_help_exit=0を記録できた場合だけ、既存driverによる隔離受入れへ進む。root実行やmode bitの机上計算で代替しない。旧stage.pyをそのまま再実行しない。
6. 隔離受入れに合格した場合だけ、実NICへBANなしで一時装着し、新規SSH・mode・停止解除・route／TC／MTU保全を確認する。失敗後にgeneric指定・MTU／driver交換・sandbox解除で通さない。

新VMでの呼出し例。今回これを実行していない。

```sh
sudo /usr/bin/python3 -I -B /run/veil-gcp-xdp-20261027/access-check.py \
  service-check /run/veil-gcp-xdp-20261027 \
  8852a7f835ff8673c45e7a13f990f919f62121d47d5874823d4469bd74be5e4e
```

このunitとnative --helpはGCPで未実行。ローカルARM64で確認できたDACを、systemd／x86_64／XDPの合格に読み替えない。

## 費用・時間・停止条件

**新しい一台・一回、全体最大60分、運用上限US$0.20、税前計画額US$0.06。** 既に終了した枠・残時間を再利用しない。無料枠・credit・割引は計算に入れない。

| 一時間分の保守的な内訳 | 税前USD |
| --- | --- |
| e2-small | 0.016752855 |
| 外部IPv4 | 0.005 |
| 20GiB pd-standard | 約0.001096 |
| 外向き100MiBの予備（高めの参考0.23/GiB） | 約0.022461 |
| 少量log等の予備 | 0.005 |
| 合計・切上げ | 約0.05031 → 0.06 |

[Compute公式料金](https://cloud.google.com/products/compute/pricing/general-purpose?hl=en)・[disk公式料金](https://cloud.google.com/compute/disks-image-pricing?hl=en)・[IPv4／通信公式料金](https://cloud.google.com/vpc/network-pricing?hl=en)。2026-10-27に公式情報を再照会。税・換算・確定請求は別で、運用上限はGCP account全体への即時強制capではない。実行前に変化を照合し、この枠へ収まらなければ作成しない。

T0は承認・最終preflight後、最初の新設操作直前に一度だけ記録。VMにT0＋45分のterminationTime／DELETE、boot autoDelete=trueを要求・読戻しする。設定を確認できなければguest試験を始めない。T0＋20分までに新規SSH・sudo・guest準備ができなければ撤去へ進み、T0＋60分までの撤去照合を目標とする。自動削除の開始・完了遅延とAPI障害の残余リスクは前回案と同じ。

NIC送受信累計80MiBで新規case停止、100MiBを作業上限とする。周期観測で、packet単位の強制capではない。VMでbuild・OS upgradeはしない。CLOCK_BOOTTIME絶対期限・有限TTL・256件満杯時拒否・古いinstance／revision拒否・restartでBAN0を変更しない。

作成要求は一回、SDK等の自動再送なし。503・不明結果は読取りで状態を確定し、再作成しない。事前確認・隔離・実NICの失敗でも新しいcaseを止め、証拠退避と撤去へ進む。同じ枠で修正してやり直す案ではない。

## 撤去・完了判定

今回のVM／disk内の未退避データは、手動・自動DELETEで不可逆に失われる。この削除を含めた新規承認を求める。

所有Transient／pathを照合して今回のaccess／daemon／HTTPS／runnerを停止し、新規SSHと所有XDP解除を確認。秘密鍵を除く証拠を退避・hash照合後、作成応答ID・owner marker・creation time・baselineとの差・依存を照合して、今回VM→boot disk不存在→SSH rule→subnet→VPC／自動routeの順で撤去する。ephemeral IPv4解放、今回対象の不存在、既存九区分の選択項目一致を確認。未知の同名物・依存は削除しない。

SSH喪失時はcontrol planeから今回VMだけを停止／削除する。API障害で撤去を確認できなければ、費用停止未確認と所有IDを報告する。自動DELETEはVPC等まで撤去する保証ではない。

一台適合ゲートの合格は、service-check、隔離機能、実NIC装着中の新規SSH、停止解除、保全・撤去まで。成功後も公開18443・独立正常client・公開証明書を含む公開六条件は別承認で残る。計画時はこの新設対象・US$0.20・60分・削除／撤去への承認待ちだった。その後の実結果は冒頭の報告を参照。新しい試験は未承認で、この枠を自動延長しない。
