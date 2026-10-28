# GCP一台：現行XDPの適合確認・新規承認前案

2026-10-27 JST。以下は承認前に作成した計画を保全したもの。**後続の本人明示承認で一回実行し、VM・SSHまで成功、試験packageの配置権限で製品起動前に停止した。約6分6秒で新設物を撤去し、既存九区分一致を確認。今回枠は終了、再試験への許可ではない。** [実結果・失敗原因・撤去](../review/2026-10-27-GCP-VM-BOOT-AND-EXEC-PERMISSION-FAILURE.md)。

最終ゴールは、小規模公開サイトへの安全な限定試験導入。現在のマイルストーンは、現行製品が対応するGCP構成を一つ特定すること。今回の案は、**同じbinary／objectの隔離受入れ、GCP仮想NICへの一時装着・管理SSH維持・解除、撤去まで**。公開サイトの一件BAN試験ではなく、TCP18443を公開しない。

## 1. 何を一回で判断するか

1. 以前503だったprojectで、指定した一台のVM作成が成功するか。
2. 同じ製品・object・host非依存driverが、guestのkernel・権限・libcで動作するか。
3. 指定したVirtIO-Net／MTU1460の仮想NICへ既存programを置換せず装着でき、装着中も新規SSHが通り、終了時に所有linkが外れるか。
4. 既存resourceを保全し、新設物だけの撤去と証拠退避を完了できるか。

VM内にさらにVMを作る試験ではなく、Linuxのnetwork namespace／vethを使う。nested virtualizationを有効化する前提は置かない。

zone UP・quota余裕・image READYは今回読み取ったが、作成503や実XDPの解消は未確認。[対応表と読取り結果](SUPPORTED_ENVIRONMENTS.md)を使う。作成権限・組織policy・guest状態も、未確認を合格にしない。

## 2. 固定する対象

| 項目 | 承認を求める予定の値 |
| --- | --- |
| project | veil-x-tests。共有CLIのdefaultを変更しない |
| zone／machine | us-central1-b、e2-small一台、STANDARD。2 vCPU shared-core／2GiB。別zone・別機種の追加なし |
| image | ubuntu-os-cloud／ubuntu-2404-noble-amd64-v20261024、ID4575951068918267275。作成前に同一ID・READY・非deprecatedを再確認 |
| VM／boot disk | vlb-gcp-20261027-target各一件。20GiB pd-standard、boot autoDelete=true。追加disk・snapshot・backupなし |
| VPC／subnet | 新規vlb-gcp-20261027、custom、MTU1460／新規vlb-gcp-20261027-us-central1、10.77.27.0/28、IPv4-only。内部IP10.77.27.10 |
| NIC | 一個、nicType=VIRTIO_NET、ephemeral外部IPv4一個。予約IP・NAT gateway・LB・第二NICなし |
| SSH rule | 新規vlb-gcp-20261027-ssh、専用target tag、Macの確認した有線IPv4/32→TCP22だけ。正確な値はprivate manifest。変更していたら保存せず再確認 |
| 公開しないもの | TCP18443・80・443・IPv6。DNS・公開証明書の発行なし |
| 身元・保護 | VM service account／scopesなし。Secure Boot・vTPM・integrity monitoring維持。IAM・API・課金設定の追加変更なし |
| 管理鍵・ユーザー | 今回だけのSSH鍵をMacの新規private artifacts配下のssh/内へ生成し、VM固有metadataへ公開鍵だけ設定。guest試験ユーザーは新VM内だけのveiltrial、sudo利用。run専用known_hostsを使い、共有SSH config・既存authorized_keys・project共通鍵を変更しない。秘密鍵をVMへ送らない |
| 認証の確認 | project-wide SSH鍵を遮断する予定。既存のOS Login／組織policyと矛盾するなら作成前に停止し、policyを解除して通さない。host公開鍵fingerprintをcontrol planeのserial等で照合できなければSSHへ進まない |
| guest新設先 | 今回VM内の新規/run/veil-gcp-xdp-20261027/以下。証拠と試験TLSを含む。他hostのfixtureやUIDを流用しない |

既存default VPC／42 subnet／4 firewall／43 routeは変更しない。今回VPCに自動生成されるdefault internet routeとsubnet route等は新設依存としてIDを記録し、撤去対象に含める。VPC Flow Logs・Firewall Rules Logging・Ops Agent・Cloud Storage bucket・Cloud Scheduler・IAM roleは追加しない。

Google公式資料では、第一・第二世代machineのdefaultはVirtIOで、第三世代以降はgVNICのみとなる。今回はx86_64・非ConfidentialのE2でVirtIOを明示する。[NICの仕様](https://docs.cloud.google.com/compute/docs/networking/network-overview)。新VPCにMTU1460を指定し、Linux公式imageがDHCPで受け取る値をguestで確認する。[MTU仕様](https://docs.cloud.google.com/vpc/docs/mtu)。不一致ならguestのMTUやoffloadを後から変えず中止する。

## 3. 成果物と維持する契約

| 再利用するもの | SHA-256 |
| --- | --- |
| x86_64 veil-local-ban | e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890 |
| veil_xdp_kern.o | 2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f |
| 既存host非依存driver.py | fcf851556d39f399eaab2f09ae6187eac55561b3e69a882a251ebfbb9c51de00 |

今回、Macに保存済みの成果物を再hashし一致を確認した。[artifact・費用の確認](../../artifacts/gcp-xdp-feasibility-xcc4vbyo/artifact-and-cost-check.json)。新規build・Rust依存更新・loader変更は案に含めない。

- CLOCK_BOOTTIME絶対期限、有限TTL、256件上限・満杯時拒否、分割掃除、instance／revision、結果不明の再送禁止、restartでBAN0を維持する。
- 既存のXDP用profileを使用し、起動後の非root・全thread capabilityなしを確認する。nft-owner用の権限profileへ差し替えない。
- 試験daemonはRestart=no・RuntimeMaxSec180秒・停止猶予10秒。隔離HTTPS／runnerは240秒。既存driverの承認範囲にあるrestart／SIGKILL caseの間だけ必要な設定を使い、最終は停止・非enabledへ戻す。
- host非依存driver以外のPi専用scriptをそのまま実行しない。必要最小限のguest toolがなければ、新VM内のpython3・iproute2・curl・openssl・ethtool等だけを導入候補にし、kernel／driver交換・OS upgrade・保護機構の無効化へ広げない。
- 本体がnative／genericのどちらで装着されたかを観測する。現行flags空の挙動のままとし、失敗後にmode指定やqueue／buffer／MTU変更で再試行しない。

## 4. 新しい費用枠・時間枠

**新規の運用上限US$0.20、全体最大60分、VM一台・作成要求一回。税前計画額US$0.06。** 過去のUS$1・4時間や終了済みの一台枠を流用しない。US$100の価値再判断線も、今回の増額承認には使わない。

無料枠・credit・割引を使わず、一時間分で保守的に計算する。

| 内訳 | 税前USDの試算 |
| --- | --- |
| e2-small一時間 | 0.016752855 |
| 外部IPv4一時間 | 0.005 |
| pd-standard 20GiB一時間 | 0.0010959 |
| 外向き100MiBの予備 | 約0.022461。高めの参考単価0.23/GiBで計算。実宛先の一律単価という意味ではない |
| 少量log等の予備枠 | 0.005。実測の請求額ではない |
| 合計と切上げ | 約0.05031 → 計画額0.06 |

[Compute公式料金](https://cloud.google.com/products/compute/pricing/general-purpose?hl=en)・[disk公式料金](https://cloud.google.com/compute/disks-image-pricing)・[IPv4／通信公式料金](https://cloud.google.com/vpc/network-pricing?hl=en)を2026-10-27に確認。税・支払通貨換算・確定請求は別で、実額0を約束しない。US$0.20はこの試験の運用上限で、account全体の即時・強制課金capではない。超過が見込まれる場合は新しいcaseを止め、所有確認した新設物の撤去を優先する。

### 時間と通信量

- 承認待ち中はVM・T0を開始しない。認証・資源衝突・image・/32・残予算を確認後、最初の新設操作直前にT0を一度だけ記録する。
- VM作成要求にterminationTime=T0＋45分、instanceTerminationAction=DELETEを含め、boot autoDelete=trueと共に読み戻す。期限を確認できなければguest試験を始めず撤去する。
- Googleの仕様では、自動終了は指定時刻より最大30秒遅れて開始し得る。delete開始と完了を区別する。T0＋60分までに撤去・不存在確認を終える運用案とし、障害で終わらなければ費用停止を未確認として報告する。[自動終了の仕様と制約](https://docs.cloud.google.com/compute/docs/instances/limit-vm-runtime)。
- T0＋20分までにhost key・新規SSH・sudo・guest情報が揃わなければ撤去へ進む。承認待ち・失敗・返信でT0をリセットしない。
- 新規caseはNIC送受信累計80MiBで停止、100MiBを作業上限とする。周期観測でありpacket単位の強制遮断ではない。成果物は既存の圧縮転送を使い、VMで大きなbuildをしない。

## 5. 実行順と合格・中止条件

| 段階 | 実施すること | 次へ進む条件 |
| --- | --- | --- |
| 承認後のpreflight | 対象project／principal、15件等の読取り、必要権限・policy、既存ID集合、新設名／CIDR、image、Mac有線/32、hash、時間・費用を確認 | 新設対象の衝突なし。未知のIAM／policy変更を要しない。許可範囲と値が一致 |
| 作成 | 専用VPC／subnet／SSH ruleと一台。APIに一回だけ作成要求を送る。request IDと応答・operationを記録 | 作成成功と期限・所有ID・Shielded・SAなし・NIC／MTUの読戻し |
| guest読取り | kernel、driver、libc、BPF機能・権限・lockdown、既存XDP／TC、MTU／queue等、SSH・host key確認 | 条件一致。通すためにSecure Boot・権限・driver等を緩めない |
| 隔離機能 | 同一hashの既存driverを新VMの切り離したnamespaceで実行。正常HTTPS、BAN／解除、SIGSTOP中の失効、256／257件、旧操作拒否、restart消失、停止復帰 | 各caseと終了監査成功。HTTPSは専用試験CAを明示信頼する範囲 |
| 実仮想NIC | VM内部IPv4のTCP18443だけをprofileに設定。BAN対象や公開サービスは追加せず、実NICへ一時装着、Macから新規SSH、mode／FD所有を確認。停止で解除・新規SSHを再確認 | 既存program非置換、装着成功、管理継続、所有link解除、route／TC／MTU等の保全 |
| 撤去 | 証拠退避とhash照合。今回daemon停止・所有link解除後、新設VM／disk／FW／subnet／VPCと依存をID照合して撤去 | 課金対象の不存在と既存baseline保全。結果を履歴へ記録 |

503・他の作成失敗・不明結果なら再送しない。不明時はname／ID／operation／監査の読取りで状態を確定し、新しいVMや別zone・imageを追加しない。SDKの自動作成retryも使わない。guest前の失敗をXDP非対応に分類せず、その段階の未検証として終了する。

隔離失敗、実NIC装着失敗、SSH喪失、未知program・hash不一致、時間／費用／通信量の不足で中止。残り時間内に無理にcaseを詰め込まず、新設物の復旧・撤去へ進む。再試験は今回枠に含めない。

## 6. 影響・復旧・撤去対象

新VM・boot disk内の未退避データは、手動または自動DELETEで不可逆に失われる。この削除まで、実行承認の対象に含める。既存GCP resource、Pi／hp／router／Mac network、共有SSH／CLI設定は変更しない。

1. 正常なら、今回daemon／HTTPS／runnerの停止、所有XDP非装着、実NIC状態と新規SSHを確認する。
2. 証拠・command終了コード・kernel／製品log・設定前後・hashをMacの新規private artifactsへ退避する。試験TLS秘密鍵・SSH秘密鍵・OAuth tokenは退避logに含めない。
3. VM／disk label、その他のresourceのdescription等と作成応答ID・creation time・baseline差分・依存を照合する。名前だけで既存物を消さない。
4. 今回VMを削除し、boot diskの自動削除を確認する。残存したdiskは今回ID・非共有と確認できた場合だけ撤去する。ephemeral IPv4の解放も確認する。
5. 今回SSH rule、subnet、VPCと自動生成route等を依存順に撤去し、既存ID集合を再照合する。未知の依存は削除せず未完了を報告する。

SSHを失った場合は同じVMのcontrol planeによる停止／削除へ進む。guestの全firewall flush、host再起動、既存service停止で復旧させようとしない。API障害で撤去不能なら、所有ID・状態・費用停止未確認を明示する。自動DELETEは新設network等を自動撤去する保証ではない。

## 7. 完了しても残るもの

成功時の結論は、このGCP構成での隔離機能と実仮想NICの装着・管理復旧の確認まで。実NIC上で外部clientの18443をBANする試験、独立正常client、公開証明書、自然失効／restart等を同じ公開経路で揃える[六条件](DELIVERY_GATE.md)は別の承認・判定である。一台案の成功を公開pilot完了や全server対応へ読み替えない。

計画作成時は、この具体案の一台・US$0.20・時間枠・期限付き削除と撤去への承認待ちだった。その後の一回実行と終了は冒頭の実結果を参照。追加試験は未承認で、以前の実行枠・削除承認を流用しない。[試験履歴](TEST_HISTORY.md)へ戻る。
