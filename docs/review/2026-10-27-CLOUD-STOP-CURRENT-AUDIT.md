# GCP・AWS試験環境の停止状況再確認

2026-10-27 12:18 JST以降の確認。本人の「これまでのGCP・AWSを全部落とす」指示を受け、今回の試験で作った対象の現在状態を照会した。既存の別用途リソースは変更しない。追加VM・課金試験は開始しない。

## GCP：現在の不存在を確認

プロジェクト `veil-x-tests` の集約一覧をCLIで再取得した。VM・disk・予約address・router・forwarding rule・snapshotはいずれも0件。networkは既存のdefaultだけ、subnetはdefaultの42件、firewallはdefaultの4件だった。今回止めるべき試験VMや残存diskは見つからず、停止・削除APIは実行していない。

[今回のAPI応答・時刻・集計](../../artifacts/cloud-stop-current-mb1q36br/summary.json)。各照会の生応答も同じprivate artifact directoryに保全。全サービスの請求額を照合した結果ではない。

## AWS：初回照会は本人ログイン待ち（後続で解消）

CLIのSTS応答でアカウント末尾5221を確認したが、既存ユーザーinfra-managerにはEC2 DescribeInstances権限がなく、一覧はUnauthorizedOperationだった。権限追加はしていない。既存ブラウザのCloudShell経由を試みたところ、AWSコンソールのサインアウトを確認したため、ご本人へログイン・MFAの完了を依頼した。現在の稼働数を0とは断定しない。

前回2026-10-26 22:26 JSTの独立照会では、us-east-1の稼働instance・全volume・予約addressが0、試験ネットワーク類が0、専用IAM role・Scheduler group・一時inline policyは不存在だった。[前回の撤去再照合](../../artifacts/cloud-stop-audit-19nxscjc/aws-summary.json)は今回のlive確認とは分ける。

## AWS：再ログイン後の現在状態を確認、停止作業完了

2026-10-27 **12:27:03 JST**、本人の再ログイン後、既存AWSコンソールのCloudShellからboto3で読み取り照会した。STSのaccount guardで末尾5221を照合し、us-east-1の一覧は対応APIのpaginationも含めて取得した。ローカルCLIへ権限を追加せず、rootアクセスキーも発行していない。

| 確認区分 | 現在の結果 |
| --- | --- |
| EC2 instance | 全0件（起動中・停止中ともなし） |
| EBS volume／予約address | 各0件 |
| 自己所有snapshot／AMI | 各0件 |
| NAT Gateway／network interface | 各0件 |
| 試験run tagのVPC／subnet／SG／route table／IGW | 各0件 |
| 専用IAM role | NoSuchEntity |
| 専用Scheduler group | ResourceNotFoundException |
| 一時read-only inline policy | NoSuchEntity |

照合した試験runは `vlb-ng5ti4th-20261026`。既存のVPC1件・subnet6件・SG1件・route table1件・IGW1件には触れていない。試験対象の残存がないため、新たな停止・削除APIは不要だった。確認用shellをexitし、[exited]表示を確認してCloudShellパネルを閉じた。

[現在状態の照会集計](../../artifacts/aws-stop-verified-i2fsavg_/aws-summary.json)・[取得元と確認範囲](../../artifacts/aws-stop-verified-i2fsavg_/provenance.json)を保全した。前段GCP確認と合わせ、**これまでの試験対象について停止・撤去の再確認は完了**。GCPは12:18 JSTのproject集約、AWSは12:27 JSTのus-east-1と指定IAMを対象とする。別region・別serviceを含むアカウント全体の請求ゼロを保証する照合ではなく、既に発生した試験料金の請求は別途残り得る。新規の課金試験は再開しない。

今回のクラウド変更は0件。ローカルの証拠・成果物は保全。担当は単独、使用量（入力・入力内数のキャッシュ・出力）は未取得。
