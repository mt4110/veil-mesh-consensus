# GCP二台試験：作成前の構成・公開・費用・撤去案

2026-10-26 JST。Sol／High、分担なし。**設計・読取り確認のみ。今回VM、DNSレコード、証明書、SSH鍵、firewallを作成していない。作成・課金・公開・削除の承認票ではない。**

後続: 本人から承認Aを受領し、[課金前準備](../review/2026-10-26-GCP-A-PREPARATION.md)の後、[作成POSTの503で停止・新設network撤去まで実施した](../review/2026-10-26-GCP-A-VM-CREATE-503.md)。Bは未承認。今回US$1は維持し、追加のUS$100発言を予算増額とは解釈しない。以下は当初の構成・上限の計画で、成功結果ではない。

追加の本人承認後、[us-central1-bへの一回比較も同じ503で停止し、新設物を撤去・照合した](../review/2026-10-26-GCP-A-ZONE-B-COMPARISON.md)。下表の元zoneからの変更はこの一件だけで、予算・原T0・期限は維持。追加作成や公開Bの承認へ拡大しない。

最終ゴールは[限定的な試験導入の六条件](DELIVERY_GATE.md)を満たすこと。今回のゴールは、携帯テザリングを使わない二台構成と費用・終了方法を具体化し、作成前に本人が判断できる状態にすること。先行の[API有効化・quota・見積りの観測](../review/2026-10-26-GCP-API-ENABLED.md)を実通信の合格へ読み替えない。

## 推奨する構成と判断事項

**保護対象VM一台＋試験元VM一台。自宅有線のMacを管理と独立正常clientに使う。専用サブドメインへ直接HTTPS接続し、CloudflareはDNSだけを使う。** Pi・hp・routerの操作や携帯回線は不要。新しいドメインの購入も不要な候補を確認した。

前案からの重要な提案変更は二点ある。第一に、試験ポートへの許可sourceは試験元VM一件に加え、独立正常clientのMac一件が必要なので**二つの/32**とする。BAN対象は試験元VMの一件だけ。以前の「外部source一件」の承認を拡大解釈せず、公開前に実値で別確認する。

第二に、費用の残存を減らすため、**今回新設する二VMとboot diskに限り、証拠退避後の削除と絶対時刻による自動削除を作成時に承認してもらう案**を推奨する。以前のSTOPだけの案は比較履歴として保全する。自動削除では未退避データが失われ、取り戻せない。その損失を受け入れられない場合や、停止後もdiskを残す場合は、時間・費用・保全の案を再検討してから作成する。

## 1. 作成対象を固定する

| 項目 | 具体案 |
| --- | --- |
| project | `veil-x-tests`だけ。全操作でprojectを明示し、共有CLIのdefault projectを変更しない |
| VM | `veil-ban-target-test`と`veil-ban-client-test`、各一台、最大二台。MIG／autoscalerなし |
| zone | 二台とも`us-central1-a`。在庫がなければ勝手に別zone・別machineへ変更せず中止 |
| machine／OS | e2-small、2GiB RAM、標準のUbuntu 26.04 LTS x86_64。shared-coreであり性能保証ではない。family `ubuntu-2604-lts-amd64`を作成前に具体image名・IDへ解決し、同じimageを二台へ固定する |
| boot disk | 各20GiB、pd-standard、新規のみ。disk名も各VMと同名の案。新規初期化を明示し、同名diskがあれば既存boot diskとして取り込まず中止。VM削除時のauto-deleteを明示して読み戻す |
| VPC | 新規`vlb-gcp-20261026`、custom mode。既存default VPC・rule・service accountを変更しない |
| subnet | 新規`vlb-gcp-20261026-us-central1`、`10.77.24.0/28`、IPv4-only。target内部IP `10.77.24.10`、client `10.77.24.11`。他VPCとのpeering／VPN／経路接続なし |
| 外部IP | 各VMにephemeral IPv4一つ。予約IPへの昇格なし。割当後に値を固定し、VM stop/startで変わったら旧確認票を使わない |
| IAM | VMへservice accountを付けず、API scopesもなし。既定Editor account、service account key、追加IAM roleを使わない |
| SSH | Mac上の新規試験専用Ed25519鍵。public keyだけを二VMのinstance metadataへ設定し、project-wide keyを遮断。既存鍵・SSH config・known_hostsを上書きせず、試験専用ファイルを使う |
| 認証の前提 | project／組織がOS Loginを必須化していたら、それをFALSEにして迂回しない。現案を止め、必要な最小権限を別に確認する。CLIの再認証は本人操作を含む準備段階で済ませる。ADCやservice account credentialを新規発行しない |
| その他 | snapshot・backup・local SSD・Ops Agent・Cloud NAT・LB・Cloud DNS・新規有料planなし。vTPM／integrity monitoringを維持し、Secure Bootを有効にする案。失敗を通すために保護を無効化しない |

VM／diskはrun IDのlabel、network／subnet／firewallは名前・description・ID・creation timeを所有manifestに記録する。labelだけや名前の前方一致だけで所有を決めない。VPC生成に伴うroute等もbefore／afterへ含める。同名既存物や未知の依存があれば保全して中止する。

### 配布する製品とVM内の準備

手元の既存Linux x86_64 ELFとC XDP objectのSHA-256を今回再確認した。

| 成果物 | SHA-256 |
| --- | --- |
| `veil-local-ban`（先行hp試験と同一binary） | `e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890` |
| `veil_xdp_kern.o` | `2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f` |

binaryは動的linkで、GCP上のABI・kernel・NICとの互換性は未検証。配置後のhashと実行中`/proc/PID/exe`、実kernel／NIC／driver／XDP modeを確認する。新規VMに必要なUbuntu公式package（Python、curl、OpenSSL、CA、iproute、Certbot等）だけを準備し、大規模buildやOS upgradeを試験枠へ追加しない。

最初は匿名network namespace／vethで容量・期限・解除・競合・restartを確認。その後に専用VMの実NICへ進む。既存XDP／TCがあれば置換せず停止。製品のFD所有、最小権限、有限TTL、256件上限、再起動時BAN消失を維持する。現在のkernel loaderはattach flagを空で渡すため、実modeを観測し、native対応を先に断言しない。

## 2. 公開範囲と通信経路

確認票では次を実値に置き換える。**MはMacの有線en0の外向きIPv4、Cはclient VMの外部IPv4、Tはtarget VMの外部IPv4。** Mは今回en0指定のHTTPS照会で観測し、default routeも有線だった。携帯回線は使っていない。動的な自宅IPの具体値はprivate確認票へ保管し、公開向け本文へ固定しない。

| 新規rule案 | source | target／許可 | 作成・有効化時点 |
| --- | --- | --- | --- |
| `vlb-gcp-20261026-ssh` | M/32のみ | 今回二VMの専用target tag、TCP22 | 作成承認Aの範囲。公開0.0.0.0/0やIAP全範囲を使わない |
| `vlb-gcp-20261026-https` | C/32とM/32のみ | target VM専用tag、TCP18443だけ | IP・復旧手順確定後に承認B。DNS-01の発行と証明書確認が終わってから初めて有効化 |

新規custom VPCの他ingressはimplied denyに任せ、内部相互通信の全許可ruleも追加しない。IPv6／UDP／HTTP3を公開しない。18444は隔離namespaceの対照に限り、公開しない。試験server自身もMとCだけを許し、未知source・未知scopeを拒否する。sourceやtargetを省略したruleを作らない。[Googleのsource範囲・targetの仕様](https://docs.cloud.google.com/firewall/docs/firewalls)

bootstrap用の外向きpackage／CA／DNS通信と、許可済み接続への応答は必要。これは全方向を遮断したsandboxではない。自宅LANへの経路やTunnelを作らず、外向き帯域試験・scan・floodはしない。課金対策として二VMのNIC送信量を観測し、合計80MiBで新規caseを止め、100MiBを作業上限として停止・撤去へ進む。定期観測には間隔と停止処理の遅延があり、packet単位の強制上限ではない。

clientは**Tの公開IPv4**へ接続する。内部IPへの接続をWAN試験に数えない。Macも自宅有線から同じT／FQDN:18443へ接続する。targetで観測したL3 sourceがC／Mであることと、NIC位置のdestinationを確認する。GCPのNAT後は内部宛先になる想定だが、観測前に実scopeを確定しない。違ったらpublic ruleを広げず停止する。[Googleの外部IPv4とNAT](https://docs.cloud.google.com/nat/docs/public-nat)

BAN禁止はM、gateway、metadata等の管理に必要なsource。BAN対象はC一件のみ。Macの正常HTTPSと新規SSHが同じBAN中も続くことを確認する。既存SSH一本が生きているだけで管理復旧を合格にしない。

GCPのcloud control planeを復旧経路として使い、SSH不能時は今回のサービス／VMだけを停止・削除できる状態を先に確認する。SSH host keyは作成したVMのserial出力等で公開fingerprintを照合し、未知host keyを無検証で登録しない。serialの対話loginやproject metadata変更を追加しない。[VM単位のSSH鍵](https://docs.cloud.google.com/compute/docs/connect/add-ssh-keys)

## 3. ドメインと通常ブラウザ向けHTTPS

本人が利用可能な既存ドメインの利用を許可したため、Cloudflareを読取り確認した。候補は **`vlb-gcp-20261026.veil-s.com`**。`veil-s.com`のDNS画面では既存`api.veil-s.com`がveil-vaultのWorkerであり、変更対象から除外する。

今回のDNS照会では、zoneのNSはCloudflare、apexのCAA応答はNOERROR／answerなし、候補名のA／AAAA／CNAMEと`_acme-challenge`のTXTはNXDOMAINだった。これはその時点の観測で、作成直前に権威DNS・Cloudflareの両方で再照合する。既存CAA／wildcard／同名recordが見つかったら上書きしない。

| 追加するもの | 内容・保護 |
| --- | --- |
| A一件 | `vlb-gcp-20261026.veil-s.com` → T、DNS-only、TTL120秒案。値はT割当後に確認。AAAA／CNAMEを足さない |
| TXT一件 | `_acme-challenge.vlb-gcp-20261026.veil-s.com`へ、そのrunのDNS-01 challenge値だけ。既存TXTと置換・混同しない |
| 証明書 | target VMでCertbotのmanual DNS-01を用い、このFQDN一つの公開証明書を発行する案。DNS API tokenを発行・配布しない。challengeの反映はbrowserで行う |

DNS-01ではWebserverの80／443を全世界へ開ける必要がない。公開証明書発行前に規約と対象名を示して確認し、承認Bに含める。証明書名はCTへ公開され、DNSを戻してもCTの履歴は消せない。Cloudflare proxyは有効にせず、CDNのsourceを利用者の代わりにBANしない。[DNS-01](https://letsencrypt.org/docs/challenge-types/)・[Certbot manual](https://eff-certbot.readthedocs.io/en/stable/using.html#manual)・[CT記録](https://letsencrypt.org/docs/ct-logs/)・[DNS-only](https://developers.cloudflare.com/dns/proxy-status/)

既存siteの証明書・秘密鍵・origin証明書をコピーしない。試験用の新規private keyはtarget VM内の制限したpathで生成し、内容をチャット・log・Git・Macの証拠へ出さない。自動更新や常設Webserverを設定せず、packageが作る更新timerも新規VM内だけで停止する。

Webはrun IDを返す小さな専用pageだけ。管理画面、user data、DB、WordPress相当の実装は不要。通常のブラウザから`https://vlb-gcp-20261026.veil-s.com:18443/`を開き、標準の証明書検証を通す。証明書警告の迂回、curl `-k`、OS trust store変更はしない。

DNSや発行が指定枠内で成立しなければ、その公開受入れは未達として終了する。明示信頼の試験証明書を使った隔離確認まで実施できても、通常ブラウザの合格へ読み替えない。新規有料domain、80／443の許可拡張、既存CAA変更へ自動で進まない。

## 4. 四時間の費用と、US$1の意味

以下は二台を各四時間、diskも四時間で削除し、無料枠・credit・割引を使わないUSDの**計算例**。今回のrunによる増分であり、既存account全体の請求をUS$1以内にする案ではない。

| 費目 | 単価・計算 | 税前概算 |
| --- | --- | --- |
| e2-small | $0.016752855 × 2台 × 4時間 | $0.134023 |
| ephemeral外部IPv4 | $0.005 × 2個 × 4時間 | $0.040000 |
| pd-standard | $0.000054795／GiB時 × 40GiB × 4時間 | $0.008767 |
| 固定部分計 | 上の三項目 | **$0.182790** |
| 少量の通信 | 100MiB全量に$0.12／GiBを当てる計算例では$0.011719。VM間外部通信の区分や実宛先も確認し、計画では通信枠$0.03を確保 | $0.03の枠 |
| 税前計画額 | 固定分＋通信枠を切り上げ | **$0.22** |

[VM料金](https://cloud.google.com/products/compute/pricing/general-purpose?hl=en)・[IPv4／転送料金](https://cloud.google.com/vpc/network-pricing)・[disk料金](https://cloud.google.com/compute/disks-image-pricing)を2026-10-26に確認。diskの作成画面の$0.00は無料枠の可能性があるが、本計算には使わない。通常版Ubuntuを選び、Pro等の追加licenseを含めない。

**US$1は今回の作業・支出の承認上限案で、GCPが全費目を必ずUS$1で止める保証ではない。** 通常のbudget alertは強制停止ではなく、現在のSpend cap対象にもCompute Engineは含まれず、継続するcompute／storage料金を止める機構ではない。[budget alert](https://docs.cloud.google.com/billing/docs/how-to/budgets)・[Spend capの対象と制約](https://docs.cloud.google.com/billing/docs/how-to/budgets-spend-caps)

請求通貨・税・実適用単価・丸めは作成直前に確認する。税込相当の増分がUS$1内と判断できなければ作らない。$0.22との差を未承認の追加VM・再試験・延長へ使わない。作業中の資源数・経過時間・送信量から保守的に推計し、増分$0.50到達または不明な課金resource発見で新規caseを止め、撤去する。請求画面の遅れを実時間の費用計と誤認しない。

停止後にdiskだけ40GiB残すと、無料枠なしでは約$0.0526／日が続く。cloud障害・制御不能・削除失敗による超過の絶対保証が必要なら、この案では課金resourceを作らない。budget設定のために既存請求accountや他projectを停止する機構も作らない。

## 5. 時間・停止・撤去を先に固定する

T0は最初の新設resource作成要求を送る直前（VPC作成も含む）。作成前の準備段階でCLI再認証、所有確認、入力manifest、準備物、driverのオフライン検証を済ませる。以後の待ち時間を含めて四時間以内とし、二台とも同じ絶対時刻を使う。

| 時点・期限 | 動作 |
| --- | --- |
| T0〜60分 | 試験portを閉じた状態で二台作成、hash／権限／NIC／隔離受入れ、DNS-01 clientの準備。実際の規約同意・challengeレコード・発行は承認Bの範囲。60分以内に公開前提と承認が揃わなければ公開caseを始めず終了手順へ |
| 公開前 | C・T・M、FQDN、A／TXTの値、公開rule、case列、残時間を承認Bで確定。サーバー／daemonの180／240秒枠は承認後だけ開始。待ちが長ければサービス期限を延ばさず公開前に中止 |
| 公開case | 共通の180秒guard、daemon最大180秒、HTTPS最大240秒。daemon restartでも共通guardの終了時刻を延ばさない。Restart=no、一時unitのみ、enableなし |
| T0＋3時間まで | 新しいcaseを終了。以後は復旧・証拠回収・撤去だけ。遅くともこの時点で今回のDNS Aを削除し、TTL120秒の経過と権威／再帰DNSの状態を確認してからIPを解放する |
| T0＋3時間30分まで | 新VM／diskの削除と新network対象の撤去、所有manifestとの照合を終える目標。自動削除を待って通常の終了作業を省かない |
| T0＋3時間45分 | 二VMに作成時設定した`terminationTime`、action=DELETEによる予備の自動削除。boot diskはautoDelete=true。再起動で期限を後ろへずらすduration型を使わない |
| T0＋4時間まで | 残存と費用対象を再確認。未達なら試験不合格／撤去未完了として報告し、新しい試験はしない |

時刻指定終了は開始に最大30秒の遅れがあり、完了時刻の保証ではない。VMが既に停止していると予定終了timestampは消えるため、**STOPしたdiskが後で自動削除されるとは考えない。** 手動stopや異常停止後は、所有確認したVM／diskを同じ終了手順で削除する。Macからの読取り確認とGoogle側の期限を併用するが、二つあれば障害がなくなるとは説明しない。[期限と再起動時の動作](https://docs.cloud.google.com/compute/docs/instances/limit-vm-runtime)

### 正常終了と異常終了

1. 新規変更を止める。結果不明のBAN／unbanを再送せず、今回のdaemonだけを停止。所有XDP解除・新規SSH・HTTPS復帰を確認する。自分のXDPでないものは外さない。
2. 今回のHTTPS／guardを停止し、試験port・process・一時unit・enable状態を確認。新規18443 ruleを無効化する。管理SSH ruleは証拠回収まで残す。
3. 非秘密のcase結果、実binary／object hash、kernel／NIC／XDP、counter、instance／revision、before／after、所有manifestをMacのprivate artifactへ取得してhash照合する。各case後にも小さく退避し、最後だけの回収に依存しない。
4. 今回追加したAとchallenge TXTだけを、record ID・名前・値・run IDで確認して削除。`api.veil-s.com`、既存record、zone、nameserver、SSL設定、Workerには触れない。公開証明書のCT履歴は残り、削除では戻せない。
5. 今回新設したVM二台と各boot diskだけをmanifestのIDで照合して削除する。追加diskや外部依存を発見したら強制削除せず保全して止める。ephemeral IP解放を確認する。snapshotで保全して費用を残す方式へ勝手に切り替えない。[VM削除とdisk](https://docs.cloud.google.com/compute/docs/instances/deleting-instance)
6. 新規HTTPS／SSH rule、新subnet、新VPCとその所有routeを撤去する。共有・default対象は除外。依存が見えたらforce／recursive削除を使わない。
7. 請求対象が残らないことをresource一覧で確認し、利用料金の反映後の照合は未確認なら未確認と残す。新しいSSH鍵はcloud側のinstance metadata消滅で失効し、Macのprivate記録は保全する。local fileを自動削除しない。

SSHを失った場合も、初手は今回の公開rule閉鎖とcloud APIによる対象VM停止・削除。所有不明や別projectを触って復旧しない。証拠を回収できないまま期限で削除された場合は「証拠未回収」とし、成功にしない。DNS削除が未達なら古いAが他利用者へ再割当されたIPを指す残余リスクがあるため、明示して復旧を継続する。

## 6. 実施するcaseと合格判定

隔離段階では既存の一覧／preview・容量256／257件・TTL・解除・古いinstance／revision拒否・通常／SIGKILL restartを新VM上で確認する。新NICの受入れと別に記録する。破損・256件満杯等の障害注入を公開入口で行わない。

公開段階は次の固定列を一度だけ実施し、原因不明の同条件反復をしない。各BANはC一件、TTL30秒、新しいpreviewを別の明示操作として適用する。

| case | 必要な証拠 |
| --- | --- |
| 基礎防御／空BAN | CとMから同じFQDN:18443へ正常TLS・期待body。実sourceと変換後scope、Macからの新規SSHを確認 |
| 一件BAN・明示解除 | Cの新規対象接続だけ遮断、XDP counter増加。Mの同じHTTPSと新規SSH継続。元期限前の解除とCの通信復帰 |
| 別の一件BAN・自然失効 | 新たな有効BANの遮断を先に確認し、元の30秒で復帰。延長や再BANを失効の代わりにしない |
| 別の一件BAN・通常daemon restart | 有効BANを確認後、新instance／BAN0と通信復帰。旧instance拒否は隔離証拠とも対応付け、公開側で確認するなら同じCだけを対象とし、期待拒否でなければ即停止 |
| 終了 | daemon停止・所有XDPなし・正常通信復帰、その後Web停止と公開rule閉鎖。本人の通常ブラウザでの証明書／page確認と、運用手順で迷った点も別に記録 |

public IP宛て接続、独立正常client、通常ブラウザの信頼、運用者の実操作のうち未達があれば、その欄を残す。公開証明書だけやcurl成功だけで全て完了にしない。今回が通っても、そのimage／kernel／NIC／小さな手動試験の範囲であり、性能・DDoS・全cloud・大規模運用の保証ではない。

## 7. 作成前に本人が確認する範囲

**承認A（10-26受領、実施状態は後続記録を参照）:** 上記の名前・project・zone・二VM／二新規disk・専用VPC／subnet・M/32だけのSSH rule、Mac専用鍵、必要最小限の新規VM内準備、最大四時間・増分US$1の運用上限、証拠退避後／期限での新設VM・disk削除、今回新設network対象だけの撤去。未退避の新規VM内データは失われる。既存resourceを削除対象にしない。AだけではTCP18443の公開、DNS保存、証明書発行、実NICの公開BANを始めない。

**承認B（Aの後、IP確定時）:** C/32とM/32→T:18443、DNS A／TXTの実値、新FQDN一つの証明書発行・規約・CT公開、固定した三つのBAN case・解除／失効／restart、サービス期限、DNS／ruleの復旧と新設record削除。値・対象・残時間が変われば旧確認を流用しない。

証明書の規約確認→同じ発行要求からのchallenge生成→DNS保存→発行確認→port有効化の依存順を守る。TXTは生成後の値を表示して照合し、まだ生成していない値を確認済みにしない。browserでの保存時に別の確認が必要になれば、その対象だけを示す。待ちの間にサービスを起動したり、四時間枠を延長したりしない。

作成後に必要になるdynamicなIPやDNS challengeを、今回の空欄のまま承認済みとしない。一方で同じ承認済みのcase内の通常の読取り・停止・復旧を、理由なく再確認しない。既存リソースに影響する追加操作や、予算延長は含めない。

今回のprivate観測・計算票は`artifacts/gcp-two-vm-plan-njnzp3bz/approval-values.json`。これは作成済みresourceのmanifestではなく計画票で、resource IDはまだない。[試験履歴](TEST_HISTORY.md)へ結果を追加し、実施した後に実際のID・結果・未達・撤去を別報告へ残す。
