# GCP専用一台：明示Generic・offload保全の試験案

後続: Aの本人承認で[新x86_64版・専用package・helperを準備](../review/2026-10-27-GENERIC-X86-PACKAGE-PREPARATION.md)した。hashは固定済み。QEMUでRust kernel GET一件が失敗するため全回帰合格とはせず、同じRust回帰実行ファイルを同梱してGCPの最初のnative gateで全51合格を要求する。未実施と書かれた以下の準備節は計画時の記録として保全する。BのVM作成・NIC変更・実XDPは未実行。

2026-10-27 JST。Sol／High、分担なし。**未承認・未実行の案。今回行ったのは公式資料・既存code／証拠・GCPの読取り確認と文書化のみ。VM作成、build、package改修、NIC設定変更、課金試験は行っていない。** 過去の試験枠・作成承認を再利用しない。

最終ゴールは、普通のブラウザが使う小規模公開サイトへの安全な手動一時BANの限定導入。現在のマイルストーンは、新しいguard付きloaderがGCPの専用VirtIO NICで安全に装着・終了できるかの判定。今回は、その一台・一回の作成前案を費用・変更・復旧・撤去まで具体化する。[新loaderの非課金検証](../review/2026-10-27-GENERIC-OFFLOAD-GUARD-IMPLEMENTATION.md)と[前回の実NIC EINVAL](../review/2026-10-27-GCP-ISOLATED-XDP-PASS-NIC-EINVAL.md)を前提にする。

**推奨：課金前の成果物準備 → hashを提示して実行承認 → 一台で隔離機能・明示Genericの通常停止／SIGKILLを判定 → 復元・撤去。** 今回は公開18443やWANのBANを含めない。この試験だけでは[公開六条件](DELIVERY_GATE.md)の達成にならない。

## 1. 検証する仮説・終了条件

「専用のplain VirtIO NICで、LRO／hardware GROのactive・wantedを事前にoffへ構成した後なら、同じFD所有方式の明示Genericで装着でき、新規SSHを維持し、通常停止・SIGKILL後にlinkを残さず、off状態も変えない」を検証する。

VM作成要求は一回、実NIC装着は**予定した二case各一回**（正常終了・SIGKILL）だけ。二回目は一回目の完全合格後だけ実施する固定の障害caseで、失敗後の再装着ではない。native／defaultへの変更、fallback、条件緩和、同じ枠での修正・再作成はしない。実NICにはBANを入れず、機能的BANは接続のない新規namespace内だけで検証する。

## 2. 読取りで確認できた現状

[今回の読取り証拠](../../artifacts/gcp-generic-plan-zoy0it5b/)を保存。全てexit0。実行直前には再確認する。

| 項目 | 2026-10-27の確認 |
| --- | --- |
| project | veil-x-tests。CLIは引数で明示し、共有default project設定は変更しない |
| 既存resource | VM0、disk0、予約address0、router0、forwarding rule0。default VPC1、subnet42、firewall4、route43が存在。これらは全て保全対象 |
| 固定image | ubuntu-os-cloud／ubuntu-2404-noble-amd64-v20261024、ID4575951068918267275、X86_64、READY |
| machine／zone | us-central1-bのe2-smallを参照可。guestCpus2、memory2048MiB。作成成功や空き容量の保証ではない |
| 管理source | Macの有線en0、IPv4 111.217.200.148。tethering不使用。実行前に変わっていれば新値の承認前に作成・rule保存しない |

選択項目の一覧であり、全service・全regionの利用料金0や、account全体の無課金を保証する照会ではない。

## 3. 新設対象・変更範囲

| 対象 | 固定案 |
| --- | --- |
| project／zone／VM | veil-x-tests／us-central1-b／vlb-gcp-generic-20261027-target、e2-small、STANDARD一台 |
| boot disk | 同VM名、20GiB pd-standard、boot autoDelete=true。snapshot／追加diskなし |
| VPC／subnet | 新規vlb-gcp-generic-20261027／vlb-gcp-generic-20261027-us-central1。custom、MTU1460、10.77.28.0/28、IPv4-only |
| NIC／IP | VIRTIO_NET一つ、内部10.77.28.10、ephemeral外部IPv4一つ。予約IPなし |
| ingress | 新規vlb-gcp-generic-20261027-sshだけ。111.217.200.148/32→対象VM tagのTCP22のみ。0.0.0.0/0、18443、80、443、IPv6の受信ruleなし |
| 身元・保護 | SA／OAuth scopeなし。Shielded Secure Boot／vTPM／integrity monitoring維持。新runだけのSSH鍵・known_hosts、VM metadata。OS Login／組織policyを無効化しない |
| guest配置 | /run/veil-gcp-generic-20261027/だけ。binary等はroot所有0755／0644、秘密鍵はpackageに含めない。control dirはcaseごとに新規、service UIDだけ書込み可 |
| 一時unit | 同prefixのaccess、runner、isolated-daemon、nic-normal、nic-kill。Transient／非enabled。専用DynamicUser、製品は起動時だけ従来の三capability、READY後は全threadで権限0 |
| 時間制限 | access15秒、製品各180秒、隔離runner／HTTPS240秒、停止猶予10秒、起動上限10秒。Restart=no。既存隔離caseの明示的restartのみ維持 |
| offload変更 | **この新VMの検証したens4／ifindexだけ**。必要なLRO／hardware GROのwantedをoffへ変更し、最後に保存したwantedへ戻してactiveも照合する |

同名既存物・未知の依存・他管理者の変更があれば中止する。Pi・hp・router・Macのnetwork・既存GCP resource・IAM／API／請求先・常設unit・EFIは変更しない。二台目、NAT gateway、LB、Cloud Scheduler、監視agent、DNS、証明書発行、公開siteは追加しない。新VMにもbuild・apt install／upgrade・再起動を持ち込まず、必要toolがなければ撤去する。

## 4. 課金を始める前の必須準備（未実施）

**現状の成果物をそのまま使ってVMを作成しない。** 新loaderの確認済みbinaryはARM64、GCPはx86_64。既存[package helper](../../scripts/local-ban-trial-package.py)、[driver](../../artifacts/gcp-a-prep-9olhrkec/driver.py)、access／NIC helperには旧binary hashや旧固定pathがある。新sourceを試したつもりで旧binaryを動かす危険を排除する。

1. 作業ツリーのCargo manifest／lock・必要sourceをprivateな新snapshotへ固定する。commitだけで未commit差分を表現しない。旧binary・object・package・driverは上書きしない。
2. Macの既存Colima／Docker内の、**x86_64、glibc<=2.39の隔離build環境**を候補にする。Rust1.98.1系列、Ubuntu24.04と互換なsysroot（例：Debian12 bookworm）、image digest・toolchainを固定。AMD64実行／cross環境の利用可否は未確認。既存機構で実行できなければこの段階で保留し、hostのbinfmt設定・hp／Piへのbuild移転・有料VM上のbuildを自動追加しない。新imageの取得等が必要なら対象・容量を先に示す。
3. 新規非root container、cap-drop=ALL、NoNewPrivs、read-only rootfs、ソースと公開crate cacheの必要部分だけを読取り共有、新規targetを使用。build／回帰はoffline・locked。既存データ・認証directory・Docker socketはcontainerへ渡さない。成果物と停止containerは削除せず保全する。
4. ELF architecture、interpreter、GLIBC要求版、同hashの非root実--help、serve --helpのgeneric選択肢、library回帰を確認する。ARM64版やglibc2.39超のnative host buildをそのまま配布しない。
5. **新しい試験packageを別名で準備**する。旧driverの機能case・TTL・待機時間・判定は変えず、新binary hash／新path／明示--xdp-mode genericだけを適合させる。profileの180秒／240秒・権限制限を維持。archiveの全member・単一link・root所有・0755／0644・hash・非rootアクセス回帰を再検証する。
6. 試験用のboundedなfeature GET記録を用意する。GSSET_INFO／GSTRINGS／GFEATURESから**全feature名・available・wanted（requested）・active・never_changed**をJSONに保存する。製品guardと同じUAPIを照合し、欠落／壊れた返信を拒否。ethtool -kの見た目だけをwantedの証明にしない。helperは試験用で、製品CLIや定期監視を増やさない。
7. helperのpreflight／SET選択／復元・owner照合・時間残量・失敗分岐、二caseの固定順序をoffline検証する。SETは対象二featureの必要差分に限り、JSONからshell文字列を実行しない。課金前準備では実NICのSET／XDP装着をしない。
8. 新x86_64 binary／同じBPF object／全helper／archive／manifestのhash、profile、費用・source /32を提示し、**課金・NIC変更・期限付きDELETEを含む具体的な実行承認**を得る。hash・環境が未確定ならT0を開始しない。

現在のobject SHA-256は2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2fで維持する。新x86_64 binary／package hashは未確定。今回の文書作成は上記の実装・取得・buildへの自動着手ではない。

## 5. offloadの事前設定・影響・復旧

### 読取りと事前条件

新VMのdefault routeが一つ、ens4／同じifindex／virtio_net／MTU1460／内部10.77.28.10であること、master／下位deviceなし、既存XDP／TCなし、qdisc最大16のguard適合、他のfeature設定managerなしを確認。kernel package revision／version_signature・driver・VirtIO feature・queue・ethtool版・offload全体を保存する。取得不能を推測でoff扱いにしない。

`B0`を設定前の全featureのactive／wanted／available／never_changedとする。同じ状態を再読取りできること、kernel・boot ID・netns・ifindexの一致を確認する。LRO／GRO_HWが既にactive=0・wanted=0なら、そのfeatureへのSETは省略。固定offも正常。on／wanted-onのfeatureが変更不能・未知なら、他方も変更せず試験を中止する。

### 変更する操作

管理者が今回の試験手順として一度だけ実行し、製品はGETだけを行う。[ethtoolのfeature操作](https://man7.org/linux/man-pages/man8/ethtool.8.html)は-Kで指定する。次は対象値の**例であり未実行**。実際はB0から必要な項目だけを選ぶ。

```sh
# LROだけのactive/wantedがonで、変更可能な場合
sudo ethtool -K ens4 lro off
# hardware GROだけのactive/wantedがonで、変更可能な場合
sudo ethtool -K ens4 rx-gro-hw off
```

activeだけを根拠に省略しない。wanted-onlyのonも処理する。二featureとも変更が必要な場合は一つの引数列にまとめ、部分失敗も成功とみなさず直ちに復旧・撤去へ進む。offにした後の`B1`で両featureのactive／wantedが全て0、他featureはB0と一致、新規SSH正常を要求。MTU・queue・software GRO・checksum・route・firewall・driverは変更しない。

**影響:** このVMの全受信通信、SSHを含むoffloadの集約処理が変わり、CPU負担増加・throughput低下・一時的な通信断があり得る。明示Genericも全受信packetを通り、設定保全のGET→装着は他の特権操作に対して原子的ではない。専用・無負荷・同時再構成なしに限定する。性能改善や他hostへの無影響を測定したことにはしない。

### 停止と復元

`B1`は**試験中に保全すべきoff状態**、`B0`は**最後に戻す元の状態**と分ける。通常停止後とSIGKILL後にlink不在・全active／wanted=B1をそれぞれ確認する。daemonの自動再起動で代用しない。

全製品unit停止・所有link不在を確認してから、同じboot／netns／ifindex、無関係なfeature差分なしを再検査し、**変更した二featureだけのwantedをB0へ戻す**。元からoffだったfeatureにはSETしない。例：B0のhardware GROのwantedがonで、それだけをoffにしたなら`sudo ethtool -K ens4 rx-gro-hw on`。元のwantedとactiveが異なる場合、activeをwantedへ写して復元したことにしない。最後に全active／wanted=B0と新規SSHを確認する。

未知の差分・別processの変更があれば、自動で全featureを巻き戻さない。復元失敗、SSH喪失、所有linkが残る場合は試験を停止し、Mac側control planeから**今回作ったVMをID照合して削除**する。offload未復元は失敗として記録する。専用VM削除によって今回のguest NIC／設定を廃棄できるが、「復元合格」とは扱わない。未知のXDPへのxdp off、host全体のnetwork restart／rebootはしない。

## 6. 新binaryの検証順序

| 段階 | 確認・合格条件 |
| --- | --- |
| 非root起動 | package属性／hash、capabilityなしDynamicUserの実--helpが3秒以内exit0。新generic選択肢、実ELF一致。同梱した同じRust library回帰の全51合格も必須。root実行や別のPython判定で代替しない |
| 接続のない隔離namespace | 既存32確認を同じ基準で実行。正常HTTPS／対象外port、一件BAN・preview解除、元のTTLでの失効・SIGSTOP中の期限、256／257件、古いrevision／instance拒否、通常／SIGKILL restartでBAN0、所有socket・link回収。全起動を明示Genericに固定 |
| 隔離の終了 | runner成功、所有unit停止・非enabled、子process回収、所有namespace／veth消滅。guest実NICがB0のまま。失敗時は実NIC設定へ進まない |
| 実NIC事前設定 | 前節のB0→B1、両featureのactive／wanted off、他feature不変・新規SSH正常 |
| G1：通常停止 | nic-normalの空BANでens4へ明示Generic一回。内部10.77.28.10:18443をscope、管理source保護。READYのrequested／actual generic、program ID・FD所有・実mode2、全thread非root・権限0、別の新規SSH、B1一致。SIGTERM停止後link不在・B1一致・新規SSH |
| G2：SIGKILL | G1完全合格後だけ別unit／新control dirで同じ空BANを一回。別の新規SSH、B1一致を確認してMainPIDの同一性を照合しSIGKILL。Restart=no、link不在、unit非active、B1一致、新規SSHを要求。失敗したG1の再試行には使わない |
| 復元・終了 | B1→B0を確認、証拠退避・hash照合、全unit停止・非enabled、今回resourceの撤去・独立した不存在確認、既存選択項目保全 |

外部firewallはTCP22だけのまま。18443／18444のHTTPSは隔離namespace内のみ。実NICでBANコマンドを送信せず、管理元の遮断を試さない。二つの実NICcaseは各180秒・停止10秒以内、時間切れは不合格。公開HTTPS・独立正常WAN client・利用者向けBANは未検証として残す。

## 7. 時間・費用・自動終了

承認待ち・build・offline検証・hash確定・ログインはT0の外側。**新設操作直前にT0を一度だけ記録**し、VM作成要求は一回。曖昧なtimeout／503は読取りで存在を確定し、再送しない。新設物の撤去だけを行う。

| T0からの時刻 | 上限・停止条件 |
| --- | --- |
| 0〜12分 | 新設network・VM、期限設定の読戻し、SSH／sudo／tool確認。12分で未完なら新規caseを止め撤去 |
| 12〜20分 | package配置、非root起動、240秒以内の隔離機能・回収。20分で未完なら撤去 |
| 20〜25分 | B0取得・必要なoff設定・B1確認。25分までにG1を開始できなければ実NIC試験を始めない |
| 25〜32分 | 固定したG1／G2だけ。通常停止・SIGKILLを独立判定。失敗時に後続caseへ進まない |
| 32〜40分 | offload復元、証拠退避、hash照合。40分以降は新規caseなし、撤去のみ |
| T0＋45分 | GCP側terminationTime／instanceTerminationAction=DELETE。boot autoDelete=true。作成応答とGETの実値・予定timestampを照合する |
| T0＋60分 | 撤去・不存在・既存保全の確認までの運用上限。以後は試験せず、未回収物の復旧／費用停止対応だけ |

[公式の期限機能](https://docs.cloud.google.com/compute/docs/instances/limit-vm-runtime)で絶対時刻を指定し、再起動・作成遅延で期限を延ばさない。stop／start・reboot・期限変更は今回行わない。自動終了の開始は最大30秒遅れ得て、削除完了や関連networkの撤去まで保証する機構ではない。設定の読戻しができなければguest試験を始めない。

**最大60分・追加費用の運用上限US$0.20、税前計画額US$0.06。** free tier／credit／割引を使わず、一時間分で保守的に計算する。

| 内訳 | 一時間分の税前USD |
| --- | --- |
| e2-small、us-central1 | 0.016752855 |
| ephemeral外部IPv4一つ | 0.005 |
| 20GiB pd-standard、0.000054795/GiB-hour | 0.001095900 |
| 外向き100MiB予備、参考上側0.23/GiB | 0.022460938 |
| 少量log等の予備 | 0.005 |
| 計算合計→計画額 | 0.050309693 → **0.06** |

2026-10-27に[VM公式料金](https://cloud.google.com/products/compute/pricing/general-purpose?hl=en)・[disk公式料金](https://cloud.google.com/compute/disks-image-pricing)・[IPv4／通信公式料金](https://cloud.google.com/vpc/pricing)を再確認した。VMページ本文のfetch timeoutは保持し、同じ公式URLの検索結果で単価を照合。作成前に見積りを再照合し、確認不能・上限外なら作成しない。

NIC送受信合計80MiBで新規case停止、100MiBを運用上限として周期観測する。egressにこの全量を計上する見積りで、packet単位の強制capではない。US$0.20はGCPの即時強制課金capではなく、税・換算・請求遅延・control plane障害により厳密な絶対額／削除完了時刻を保証できない。異常時は試験を止め今回VMを削除し、停止だけでdisk費用が消えたとしない。以前のUS$100は今回の上限を拡大する承認ではない。

## 8. 撤去と保全

正常・失敗・timeoutの全分岐を同じ終了処理へ接続する。今回のunit／PID／cgroup／所有記録を照合して停止し、link解除・新規SSH・復元を確認。秘密鍵・認証情報を除くlog／B0／B1／各終了snapshot／command／ID／hashを新private artifactへ退避・照合する。

作成応答のresource ID・run label・creation time・未使用名・baselineとの差・依存の一致を必要条件として、**今回VM → boot disk不存在 → SSH rule → subnet → VPCと自動route**の順で撤去する。ephemeral IPの解放も確認し、別GET／listで今回対象の不存在と既存九区分の安定項目一致を確認する。未知の同名物や他者依存を削除しない。

手動・自動DELETEは新VMとautoDelete boot diskの未退避データを不可逆に失わせる。証拠退避が期限に間に合わなくても削除期限を延ばさず、失われた証拠は未確認とする。API障害で削除確認不能なら回収未完と報告し、追加VM・IAM変更で迂回しない。ローカルの旧証拠・build成果物・新しい試験記録は削除しない。

## 9. 承認の分け方・残タスク

| ゲート | 現在地／何ができれば終了か |
| --- | --- |
| 今回の計画 | **完了**。読取り・一次資料・既存codeから変更／影響／復旧・一台の上限を具体化。文書検査・link・既存hash確認を行う |
| A：非課金準備 | **成果物・hashと検査結果を固定済み**。helper／DAC／実--helpは合格、x86_64 QEMUのkernel GET一件は失敗を保全。全回帰合格とはしない。native判定をGCPの最初の必須gateへ持ち越す |
| B：一台の実行 | **未承認・未実施**。A完了後に確定値を確認してから作成・offload設定・固定二case・期限付き削除・撤去を承認。Aだけの承認で課金しない |
| 後続：公開受入れ | **未達・今回範囲外**。B合格でも公開六条件は別に確認し、限定導入可否を判断 |

実機枠は最大60分。Aは現在のbuild環境・helperの適合が未確認なので時間の確約をせず、環境不足なら追加構築で膨らませず保留する。Bが失敗した場合、この仮説はその条件で不合格として止め、次の有料再試験を自動追加しない。

今回のmodel／effortはgpt-6.1-sol／high、分担なし。入力・cache入力（入力の内数）・出力使用量は未取得。検査結果・読取り応答・変更前hashは[今回の証拠](../../artifacts/gcp-generic-plan-zoy0it5b/)へ保存する。追加作成・設定変更は0件。

[試験履歴](TEST_HISTORY.md)・[対応環境](SUPPORTED_ENVIRONMENTS.md)・[README](../../README.md)へ戻る。
