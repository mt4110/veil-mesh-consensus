# GCP一台：VM起動成功、配置権限で隔離試験停止、撤去完了

2026-10-27 JST。Sol／High、分担なし。[具体案](../local-ban/GCP_XDP_SINGLE_VM_PLAN_20261027.md)の一台・最大60分・US$0.20・期限付き削除と撤去について、本人の明示承認後に一回実行した。

**VM作成・公開ホスト鍵照合・新規SSH／sudo・隔離HTTPSまでは成功。製品daemonはexec時にPermission denied（203/EXEC）となった。原因は試験側の配置権限確認漏れで、root所有binaryの0700をtarで引き継ぎ、専用非rootユーザーが実行できなかった。製品code・BPF load前の失敗である。再試験・再作成・実NIC装着・BANへ進まず、08:29:12 JSTに今回新設物の撤去と既存九区分の選択項目一致を確認した。**

最終ゴールは小規模公開サイトへの安全な限定導入。今回のマイルストーンはGCPのkernel／NICに現行製品が適合するかの確認で、**未達**。承認枠の終了と撤去は完了したが、製品のGCP対応合格や公開pilot完了ではない。

## 1. 固定した対象と実行枠

| 項目 | 実行した内容 |
| --- | --- |
| project／zone／machine | veil-x-tests／us-central1-b／e2-small、STANDARD一台 |
| image | ubuntu-os-cloudのubuntu-2404-noble-amd64-v20261024、ID4575951068918267275 |
| VM／boot disk | vlb-gcp-20261027-target、20GiB pd-standard、boot autoDelete=true |
| network | 専用VPC vlb-gcp-20261027、subnet vlb-gcp-20261027-us-central1、10.77.27.0/28、IPv4-only、MTU1460 |
| NIC／管理 | VIRTIO_NET、内部10.77.27.10、一時外部IPv4。確認したMac有線source一件/32からTCP22だけ |
| 身元・保護 | service accountなし。Secure Boot・vTPM・integrity monitoring有効。新規run専用SSH鍵とVM metadataだけ |
| T0 | 08:23:06.376924 JST。最初の新設操作直前、リセットなし |
| 自動DELETE指定 | 09:08:06.376924 JST。実VMのschedulingから読戻し一致 |
| 全体期限／費用 | T0＋60分、US$0.20の運用上限、税前計画額US$0.06 |
| 撤去・九区分照合完了 | 08:29:12.435882 JST。T0から366.06秒、約6分6秒 |

作成要求はRESTでrequestId付き一回、再送なし。network／subnet／firewall／VMのoperation完了とIDを保存した。自動DELETEを待たず、証拠退避後に早期撤去した。**自動DELETE設定の読戻しは確認したが、指定時刻での実発火試験は行っていない。** [自動終了の公式仕様](https://docs.cloud.google.com/compute/docs/instances/limit-vm-runtime)では開始に最大30秒の遅れがあり得る。

今回の費用は当初の税前計画額・時間内の利用とみるが、確定請求額は未照合。US$0.20をaccount全体の強制課金capとは説明しない。100MiBの作業量上限に対し、証拠退避前のNIC送受信累計は11,346,076 bytesで、80MiBの新規case停止線未満だった。携帯・Pi・hp・router・DNS・公開証明書は操作していない。

## 2. 成功した準備と、停止した段階

作成前に九区分のbaseline、image ID、管理source、23権限、project／billing／zoneを確認した。OS Loginを無効にする変更やIAM・API・課金設定の変更は行わなかった。

今回はVM insertのHTTP200とoperation成功を得た。**以前の503の原因が分かった、恒久的に解消した、REST化で直った、とは結論しない。** 過去の失敗記録を保全し、今回の一成功を追加する。

| 段階 | 結果 |
| --- | --- |
| VM作成、期限・Shielded・SAなしの読戻し | 成功 |
| serialから得た公開ホスト鍵をrun専用known_hostsへ登録し、新規SSH・sudo -n true | 成功。host検証を無効化していない |
| guest読取り | Linux7.0.0-1014-gcp、x86_64、glibc2.39、ens4／virtio_net／MTU1460、lockdownはintegrity。必要toolは既設で、apt導入なし |
| host既存状態 | XDPなし、TC ingress／egress filterなし、失敗unit0を事前記録 |
| 同一binary／object／driverの転送hash | 成功。既存hashと一致 |
| private namespaceの既存XDPなし、HTTPS正常 | 二つの事前確認は成功 |
| 製品daemon起動 | systemdのexecで203/EXEC。製品のREADY・CLI受付へ到達せず |
| BAN／解除／期限／256件／restartのGCP機能受入れ | 未実施。AWS等の過去32確認を転記しない |
| 実NICへのXDP装着・装着中SSH・解除 | 未実施。隔離失敗時に先へ進まない |
| TCP18443の公開・公開六条件 | 未実施・HOLD |

## 3. 原因と、最小の改善点

Macの保存済みbinaryが0700、objectが0600だった。tar作成時に内容hashだけを検査し、専用service userが読める／実行できる属性まで検査していなかった。転送時にroot所有となり、guestでもそのmodeを保持した。親/runと試験dirは0755、/runはnoexecではなかった。

systemd journalはbinaryのexec自体についてPermission deniedを記録した。**実測した直接の失敗はroot:root／0700のbinary。objectの0600も非rootから読めない配置だが、その読取り失敗を実行した証拠ではない。** loader、BPF verifier、VirtIO driverのXDP処理まで到達したとは扱わない。

この確認漏れは試験側の不備。次の最小修正案は、新規の試験package内だけでroot所有binaryを0755・objectを0644に明示し、service userからの実効アクセスとservice userが書換え不能なことを、機能試験前に確認すること。秘密鍵やcontrol dirをworld-readableにする案ではなく、capability・sandbox・期限を緩める案でもない。今回のVMへchmodして再実行せず、元のarchive属性と失敗記録を保全した。[配置属性の解析](../../artifacts/gcp-xdp-live-b6psh2dz/package-permission-analysis.json)。

製品・object・既存driverは無変更。試験用profileにだけRuntimeMaxSec=180sを追加した。隔離driverのrestart／SIGKILL case用設定は既存どおりで、実NIC用はRestart=noの予定だったが実行していない。HTTPS／runner240秒、停止猶予10秒、TTL・BOOTTIME・256件・instance／revisionの条件は変更していない。

## 4. 停止・証拠退避・撤去

元driverのresultはcomplete=false、cleanup_ok=falseのまま保全した。failed状態のunitをinactiveと要求する終了監査も不合格だったためで、後続の独立監査で成功へ書き換えていない。

独立監査では所有Transient／FragmentPathを照合して今回unitを停止し、両unitのMainPID=0・ActiveState=failed・非enabled、server回収、実ens4のXDP未装着、route／TC／MTUの一致、新規SSHを確認した。failedは非稼働であり、正常終了へ読み替えない。その後VM自体を削除した。

result・command・journal等を許可リストで退避し、5ファイルのhashを照合。試験TLS秘密鍵は退避archiveに含めなかった。新規SSH秘密鍵はMacのprivate run dirに保持し、既存鍵を流用・削除していない。

| 今回新設したもの | 確認したID | 終了時 |
| --- | --- | --- |
| VM | 6736379250132278803 | 不在 |
| boot disk | 作成後のIDとVMの接続をprivate receiptに記録 | 自動削除、不在 |
| SSH rule | 2240855471243288091 | 不在 |
| subnet | 7564036069594914309 | 不在 |
| VPC | 9036534640314310197 | 不在 |

削除前にID・所有marker・依存を照合し、VM→disk不存在→SSH rule→subnet→VPCの順で撤去。今回VPCの自動生成routeも撤去された。九区分はinstances0、disks0、network1、subnet42、firewall4、予約address0、routes43、snapshot0、resource policy0に戻り、**件数と正規化した選択項目が全て開始前と一致**した。さらに別の読取りで今回五対象の不存在とVM／disk／予約address0を確認した。account全region／全service／全属性の無変更や確定請求0の証明ではない。

[実行要約](../../artifacts/gcp-xdp-live-b6psh2dz/final-summary.json)・[元の不合格結果](../../artifacts/gcp-xdp-live-b6psh2dz/guest-evidence/result.json)・[独立停止監査](../../artifacts/gcp-xdp-live-b6psh2dz/guest-evidence/guest-stop-audit.json)・[撤去と九区分照合](../../artifacts/gcp-xdp-live-b6psh2dz/cleanup-result.json)・[別の最終読取り](../../artifacts/gcp-xdp-live-b6psh2dz/independent-final-read.json)。証拠はprivate artifactsで、公開・commitしていない。

## 5. 判断と残り

**この実行枠は終了。GCPで現行製品が使えるかは未検証のまま。** 今回、VM準備可能という事実は得たが、XDP可否は判断できなかった。再作成・権限変更による再試験・他zone追加は行わない。

残りは、①配置属性と非rootアクセスの事前確認を修正、②別承認の一回で隔離機能→実NIC装着・管理維持・解除・撤去を確認、③適合した場合だけ別承認で公開六条件と導入価値を判定。①の修正案は上記に具体化したが、新たな実行枠は未承認。旧予算・残時間を使って自動継続しない。

モデル／推論: gpt-6.1-sol／high、分担なし。入力・cache入力（入力の内数）・出力使用量は未取得。今回の製品code変更なし。記録の文書検査と参照確認は[検証記録](../../artifacts/gcp-xdp-live-b6psh2dz/document-verification.json)へ保存する。

[試験履歴](../local-ban/TEST_HISTORY.md)・[対応表](../local-ban/SUPPORTED_ENVIRONMENTS.md)へ戻る。
