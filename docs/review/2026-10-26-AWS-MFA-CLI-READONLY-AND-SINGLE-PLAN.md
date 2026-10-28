# AWSの既存MFA・CLI権限確認と、一台試験の作成前案

2026-10-26 JST。Sol／High、分担なし。本人は既存AWS accountを確認し、バージニア北部で安価な一回の比較試験を希望した。最終ゴールは限定公開試験導入の可否判断、現在のマイルストーンはLinux VM一台での適合確認。今回はMFA確認とCLIの読み取りを完了し、作成・課金前の対象と復旧方法を示す。

**rootの既存MFA一件を確認し、本人が今回のログインで使用できたと回答。CLIの認証も有効だったが、既存ユーザーにはEC2権限がなかった。承認された一時読み取りポリシー一件でバージニアの九照会に成功し、その一件を撤去して元の二ポリシーへ戻した。VM・新しいキー・有料サービスは作成していない。**

## 認証と読み取りの実施結果

- rootのMFAはパスキー／セキュリティキー一件で、2026-02-14作成。本人の現在のログインで使用できたとの回答を確認した。こちらで追加・削除・再登録・コード入力はしていない。登録用QRや秘密は取得していない。[AWS公式の推奨](https://docs.aws.amazon.com/IAM/latest/UserGuide/enable-mfa-for-root.html)。
- MacのAWS CLIは2.36.50。default profileのSTS本人確認が成功し、ブラウザーと同じaccount（末尾5221）だった。既存CLIのdefault regionは東京だが、本人がバージニアを選択したため実照会は`--region us-east-1`を明示。共有のdefault設定を変更していない。
- 最初の東京でのEC2照会は`UnauthorizedOperation`で、identity-based policyによる許可不足と明示された。GCPの汎用503とは異なる、特定できた権限エラー。
- rootコンソールで既存の二ポリシーを確認し、本人の保存直前承認後、既存CLIユーザー`infra-manager`に`VeilEc2TrialReadOnlyUsEast120261026`だけを追加した。Describe系九アクション、`aws:RequestedRegion=us-east-1`、作成・変更・削除・IAM管理の追加なし。
- 同じCLIで九照会に成功。内容照合後に追加した一件だけを削除し、元の二件が残ることをUIで確認した。21:25 JSTのCLI照会も再びUnauthorizedOperationとなり、一時権限の撤去を独立確認した。

[追加前後・MFA確認の記録](../../artifacts/aws-existing-preflight-2ttjjm3o/permission-cycle-and-mfa-result.json)、[保存したポリシーJSON](../../artifacts/aws-existing-preflight-2ttjjm3o/ec2-readonly-us-east-1-policy.json)、[追加した状態](../../artifacts/aws-existing-preflight-2ttjjm3o/ec2-readonly-policy-added.png)、[復旧した状態](../../artifacts/aws-existing-preflight-2ttjjm3o/ec2-readonly-policy-restored.png)を保全。ポリシー名・存在の照合であり、既存二件の全文や全IAM状態を取得したとは主張しない。今回の作業用IAMタブは閉じた。

## バージニアで読み取れたこと

[九照会の結果](../../artifacts/aws-virginia-readonly-uk12e6zi/summary.json)：EC2 instance0、EBS volume0、予約address0、VPC1、subnet6、security group1。これは`us-east-1`の取得した区分で、全region・全serviceや請求の不存在を証明しない。

| 項目 | 候補として取得した値 |
| --- | --- |
| machine | t3.small、x86_64、2GiB、ENA必須、legacy BIOS／UEFI対応 |
| 固定AMI | `ami-0d6dd9e8265d48906` |
| AMI名 | `ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-20261023` |
| 所有者 | Canonicalの`099720109477`。通常版Ubuntu、Ubuntu Proではない |
| AMIのNIC／boot情報 | EnaSupport=true、BootMode=uefi-preferred、TpmSupportは未指定 |
| offering | t3.smallの取扱いが五AZで表示。実際の空き容量保証ではない |

Ubuntuのowner／name確認は[Canonical公式手順](https://ubuntu.com/aws/docs/aws-how-to/instances/find-ubuntu-images/)に従う。AMIのTPM未指定をTPM有効と読み替えず、GCP Shieldedと同等の保護を証明したとは扱わない。guestのkernel・driver・XDP・binary互換性は未検証。[ENAの公式説明](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/enhanced-networking-ena.html)・[XDP counterを含む診断資料](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/troubleshooting-ena.html)。

## 次の一回の具体案（未実施・作成前）

**一台・一回・最大60分、今回の増分作業上限US$0.20。** 対象accountは今回確認した末尾5221、regionは`us-east-1`。この案はGCPの作成承認をAWSへ流用するものではない。無料枠・creditは見積りに当て込まない。

| 対象 | 最小構成・境界 |
| --- | --- |
| EC2 | 新規`veil-one-20261026-target`一台、t3.small／Standard CPU credits、上記固定Ubuntu 24.04 AMI、AZはus-east-1aを候補。取扱い・image identityを作成直前に再確認し、失敗なら別AZや別machineへ自動移行しない |
| EBS／IP | 新規20GiB gp3のboot一枚、暗号化、DeleteOnTermination=true。ephemeral public IPv4一つ、Elastic IPの予約なし。追加volume・snapshotなし |
| 専用network | 新規`veil-one-20261026` VPC、10.77.26.0/28の専用subnet、Internet Gateway、専用route tableとassociation、security group。既存VPC・route・SGを変更せず、peering・VPN・NAT Gateway・LBなし |
| ingress | Macの作成直前確認済みIPv4一件/32からTCP22だけ。sourceが変われば旧確認値で開始しない。18443・80・443・IPv6・UDPを公開しない |
| VMの認証 | IMDSv2必須、instance profileなし。試験専用SSH公開鍵だけを配置し、秘密鍵・root access key・AWS credentialをguestへ送らない。host keyをconsole outputのfingerprintで照合 |
| 期限 | 承認・準備後、最初の新設操作直前にT0を一度記録。T0＋45分のcontrol-plane終了を設定し、＋60分までに撤去・確認。Schedulerの配信とAPI実行には遅延があり、課金を厳密に$0.20で遮断する保証とはしない |
| 終了補助 | 新規の一回限りEventBridge Scheduler schedule・専用group・専用execution role。roleは今回固有tagを持つus-east-1のinstanceへのTerminateInstancesだけ。trustをaccountと専用schedule groupに限定。作成後は対象instance IDをscheduleへ固定し、他serviceの実行権限を付けない |

制御操作は既存rootコンソールからのCloudShell短期sessionを使う案で、Macの別プロダクト用CLIユーザーへ作成権限を追加せず、root access keyも発行しない。CloudShellには今回の固有private作業ディレクトリだけを作り、既存ファイルを上書きしない。root sessionは強い権限を持つため、これは一回の隔離試験用の限定手順であり、日常運用の権限設計として採用しない。[CloudShellの認証境界](https://docs.aws.amazon.com/cloudshell/latest/userguide/cloudshell-security-faqs.html)。

VMは一回のRunInstancesだけ。Schedulerを設定・読み戻しできなければ試験を始めず、そのVMを直ちに終了・撤去する。作成失敗や結果不明時はClientToken・RequestIDと一覧を確認し、重複作成しない。既存同名物・未知の依存・所有不明のresourceを見つけたら保全して停止する。

VM本体の公開価格は一時間約US$0.021で、EBS・IPv4・少量通信を含め税前計画額US$0.08、税等の余裕を含む作業上限US$0.20を提案する。実額はaccountの料金条件・税・利用量に依存する。作成画面で超える費目があれば開始しない。[T3価格](https://aws.amazon.com/ec2/instance-types/t3/)・[EC2課金の要素](https://aws.amazon.com/ec2/pricing/on-demand/)。Unlimited credit、NAT Gateway、追加有料support、常設monitoring、domain購入を加えない。

## 試験・復旧・終了条件

1. VMが作成でき、SSH、kernel／NIC／BPF機能、成果物hashとABIを確認できるか。20分までに到達しなければ停止・撤去。
2. 同一hashの製品と既存host非依存driverを、VM内の隔離vethで確認。有限TTL、256件上限、満杯時拒否、解除、restartでBAN消失、旧instance／revision拒否を維持する。公開通信をここで合格にしない。
3. 仮想NICに既存XDP／TCがないことを確認し、保護先をVM内部IPv4のTCP18443だけに限定して一時装着。BANや公開Webを始めず、Macの新規SSH継続、停止後の所有link解除、新規SSH復帰を確認する。modeが使えなければ保護を弱めず停止。
4. 非秘密の証拠をMacへ退避し、今回のinstance終了・boot volume削除、schedule／group／execution roleの撤去、SG／route association／route table／Internet Gateway／subnet／VPCの所有ID照合による撤去と残存確認を行う。Terraform stateや既存データを削除対象にしない。

daemon180秒・隔離HTTPS240秒・有限TTL・256件上限を維持する。送信量80MiBで新規case停止、100MiBを作業上限とし、回線の負荷試験はしない。各resourceは固有run tagと作成時のIDで追跡。終了で新VM内の未退避データは不可逆に失われる。CloudShellの今回の証拠とMacの既存artifactは保全する。

今回は公開B・二台目・DNS・証明書・Pi・hp・router・Macネットワーク設定を対象にしない。成功しても公開受入れ完了ではなく、一台適合のマイルストーンまで。残りは、一台の実適合確認、承認後の限定公開受入れ、撤去・導入判定。[六つの最終受入れ条件](../local-ban/DELIVERY_GATE.md)は維持する。

[試験履歴](../local-ban/TEST_HISTORY.md)へ戻る。
