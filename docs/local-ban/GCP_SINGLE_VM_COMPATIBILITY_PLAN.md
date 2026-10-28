# GCP一台でのカーネル・NIC・XDP適合確認案

2026-10-26 JST。Sol／High、分担なし。本人の「そうしてくれ」により、二台目・公開受入れより一台の適合確認を先に行う方針を受領。**読み取り準備まで実施。元の実行枠は終了済みのため、新しい費用上限・削除期限・対象への確認前に作成しない。**

後続: 本人の「推奨で」で具体案への承認を受領し、[一回だけ実行したが24.04でも503、撤去・前後照合まで完了](../review/2026-10-26-GCP-SINGLE-VM-CREATE-503.md)。guest SSH・XDPは未実施。以下は承認前の計画を保全したもので、再実行への許可ではない。

さらに後続の本人指示で、[2026-10-27の新しい一台案](GCP_XDP_SINGLE_VM_PLAN_20261027.md)と[対応表](SUPPORTED_ENVIRONMENTS.md)を作成しました。その新案は別承認後に[一回実行、VM・SSH成功後に配置権限で停止・撤去](../review/2026-10-27-GCP-VM-BOOT-AND-EXEC-PERMISSION-FAILURE.md)しました。この旧枠を延長せず、過去の503記録も保全します。

最終ゴールは[六つの公開受入れ条件](DELIVERY_GATE.md)による限定試験導入の可否判断。今回のマイルストーンは、対応する一台のLinux VMを用意できるかの判定。完了条件は、管理SSHを維持し、同一製品の隔離確認と仮想NICへのXDP装着・解除、停止後の復旧と新設物の撤去を確認すること。公開WANのBAN合格や二台目の承認へ読み替えない。

## 読み取り準備の結果

19:56 JSTに`veil-x-tests`を明示して確認。VM0、disk0、network1、subnet42、firewall4、予約address0、route43、snapshot0、resource policy0。件数は前回撤去後と同じだが、件数だけで全設定・全サービスの一致を証明しない。[private準備記録](../../artifacts/gcp-single-preflight-umag0ryo/summary.json)に保存。

Macのdefault routeは有線en0で、同interfaceによる外向きIPv4を再確認した。今回のSSH許可元は`111.217.200.148/32`。作成直前に変わっていたら、この確認値を置き換えて実行せず停止する。携帯テザリングは使わない。

イメージ照会は最初にCLIの引数指定を誤り、構文検査で失敗した。public image照会のprojectを`ubuntu-os-cloud`として修正し、次の情報を取得した。これはイメージの読み取りで、VM作成要求ではない。最初の失敗記録も保全する。

| 項目 | 確定した候補 |
| --- | --- |
| image名 | `ubuntu-2404-noble-amd64-v20261024` |
| image ID | `4575951068918267275` |
| image project／状態 | `ubuntu-os-cloud`／READY／X86_64、通常版Ubuntu 24.04 LTS |
| machine／zone | `e2-small`、2GiB RAM、shared CPU、`us-central1-b` |

前回失敗した26.04の固定imageもREADYだった。今回は、imageを変更する一回の比較を提案し、26.04や期限指定が原因だったとは断定しない。machine、zone、boot disk、Shielded、service accountなし、期限付きDELETEの方針を維持する。新しい名前・所有ID・絶対期限は新規runの運用上の差分として記録し、厳密な一変数の実験とは呼ばない。

再利用するbinaryのSHA-256は`e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890`。GLIBC version文字列の予備走査では最大2.34だが、guest上の動的linker・実行確認に代わらない。互換性のためだけに新規buildやOS upgradeへ広げない。

## 新しい一回に限る作成・撤去の対象

| 項目 | 内容 |
| --- | --- |
| project | `veil-x-tests`。共有CLIのdefaultを変えない |
| VM／boot disk | `vlb-one-20261026-target`各一件。20GiB pd-standard、bootのautoDelete=true。STANDARD、ephemeral IPv4一個 |
| VPC／subnet | 新規`vlb-one-20261026`、custom／新規`vlb-one-20261026-us-central1`、`10.77.25.0/28`、IPv4-only。内部IP `10.77.25.10`。既存との衝突時は停止 |
| 許可rule | 新規`vlb-one-20261026-ssh`、専用target tag `vlb-one-20261026-target`、`111.217.200.148/32`からTCP22だけ。他ingressはimplied deny。TCP18443を公開しない |
| 身元と保護 | VMにservice account・scopesなし。Secure Boot／vTPM／integrity monitoringを有効。project-wide SSH keyを遮断し、専用公開鍵だけ配置。秘密鍵をVMへ送らない。host keyはserial等の公開fingerprintで照合 |
| ゲスト準備 | Ubuntu公式packageの必要最小限。既存と同一hashのbinary・object、隔離driverを今回の新規private領域へ配置。常設の公開Web、DNS、証明書、Ops Agent、backup、snapshot等を追加しない |
| 所有確認 | 作成直前のbaseline、固有run ID、実際のID・creation time・依存を記録。同名既存物や未知の依存は保全し停止 |

既存の専用試験鍵は内容を出力せず再利用する案。過去の証拠・失敗要求・helperは上書きせず、新しいrun専用の記録を使う。Pi・hp・router・Macのnetwork設定、既存GCP resource・IAMを変更しない。

## 課金・時間・停止条件

**今回追加の作業上限をUS$0.20・最大60分とする新しい枠の提案。元のUS$1枠や終了済みの四時間を自動で延長するものではない。** 課金account全体の強制上限ではなく、今回の一台と新設物に限る運用上限。JPY換算・実請求額は未確定。

一台を一時間として、e2-small $0.016752855、IPv4 $0.005、20GiB disk約$0.001096、固定部分計約$0.02285。少量の通信枠$0.03を加えて税前計画額を$0.06へ切り上げ、税・余裕を含む作業上限を$0.20とする。無料枠・creditに依存しない。[VM料金](https://cloud.google.com/products/compute/pricing/general-purpose?hl=en)・[IPv4／通信料金](https://cloud.google.com/vpc/network-pricing)・[disk料金](https://cloud.google.com/compute/disks-image-pricing)。取得・保存するログも必要な範囲へ限る。

- 承認待ちでは時計もVMも起動しない。承認後の最終preflightを通り、最初の新設操作の直前にT0を一度だけ保存する。
- VMの絶対自動DELETE時刻はT0＋45分、全体終了はT0＋60分。承認待ち・失敗・返信でT0をリセットしない。
- T0＋20分までに管理SSHと最低限のguest情報を確認できなければ、新規caseを止め撤去する。実行時に残り時間が足りなければ試験を始めない。
- VM insertは一回だけ。503・失敗なら再送、別zone・別imageの追加試行、二台目作成を行わず撤去する。結果不明なら一覧・監査で確認し、確認前の再作成をしない。
- daemonのRuntimeMaxSec180秒、Restart=no、停止猶予10秒を維持。隔離HTTPSは240秒まで、有限TTL・256件上限・再起動でBAN消失を維持する。
- 今回の送信量は80MiBで新規case停止、100MiBを作業上限とする。周期観測でありpacket単位の強制遮断とは説明しない。

## 確認の順序と採否

1. **VMとguest:** 一回の作成成功、期限・boot autoDelete・service accountなし・NICを読み戻す。SSH host keyとsudoを確認。kernel、driver、BPF機能・必要権限、既存XDP／TC、MTU等を記録。カーネルや保護設定を通すために変更しない。
2. **隔離veth:** 同一hashの製品・既存host非依存driverで正常通信、手動操作と期限失効、256件境界、restartでBAN消失を確認。private network内だけのTCP18443／対照portを使う。fixture依存・hash不一致・既存program発見等は開始前に拒否する。
3. **VMの仮想NIC:** 既存XDP／TCがないことを確認し、保護対象をVM内部IPv4のTCP18443に限って一時装着する。公開rule・Web・BAN対象は追加せず、装着中もMacの新規SSHが成立することを確認。停止で所有XDPが外れ、新規SSHが成立することを独立確認する。
4. **撤去:** 非秘密の結果とhashをMacへ退避し、今回のdaemon停止と所有link解除を確認。今回作ったVM・boot disk・SSH rule・subnet・VPCをIDで照合して削除し、課金対象の残存と既存baselineを確認する。

現在のloaderはmode指定が空で、native失敗時のgeneric再試行を保証していない。[公式根拠と実装の境界](../review/2026-10-26-GCP-EBPF-XDP-OFFICIAL-FEASIBILITY.md)を使う。装着不可なら、そのkernel／NIC／modeでの不適合として停止し、保護無効化・driver交換・製品改修で通さない。対処が必要なら、根拠と最小の別提案を示す。

今回成功しても、公開WANでのBAN、独立した正常client継続、公開証明書の六条件は未確認。二台目・公開Bへ自動で進まない。今回失敗した場合も、具体的な停止段階と復旧結果を記録して一回を終了する。

## 復旧・削除の影響

SSH不能時は今回のVMだけをcontrol planeから停止・削除し、autoDeleteと残存diskを確認する。既存resourceは操作しない。作成時の自動DELETEでもVMとboot disk内の未退避データは不可逆に失われるため、この対象の削除までを承認範囲に含める必要がある。Macへ退避した非秘密の証拠と既存資料は保全する。

期限でVMが消えてもVPC／subnet／ruleが自動で消えるわけではない。今回のIDと未知の依存がないことを確認して撤去し、不明な物は削除せず未完了と報告する。削除確認前に「費用停止」「復旧完了」としない。

[試験履歴](TEST_HISTORY.md)へ戻る。
