# 試験履歴と現在の到達点

> **2026-10-27 開発終了。** [最終終了記録](../PROJECT_CLOSURE.md)に、当初の目的の未達、クラウド・両ホストの終了確認、Piのunit・namespaceの保全をまとめました。以下は終了前の履歴です。未実行の試験案・当時のゴールを開発継続のタスクとして扱いません。

最終更新: 2026-10-27 JST。実施済みの試験、失敗・中止、設計だけの検討を辿るための索引です。元の報告・失敗記録を置き換えません。同日の行は報告書の流れに沿った区分で、個々のコマンドの厳密な実行順や総試行回数を表しません。

最終ゴールは、普通のブラウザが使う小規模公開サイトで、運用者が既知の送信元IPv4を有限時間だけ手動遮断し、安全に解除・停止・復旧できる無償OSSです。[六つの受入れ条件・試験上限・終了判定](DELIVERY_GATE.md)を使います。

**10-27の停止再確認・完了:** [GCP・AWSの現在状態を再照会](../review/2026-10-27-CLOUD-STOP-CURRENT-AUDIT.md)。GCP試験projectは12:18 JSTにVM／disk／予約IP等0を確認。AWSは初回のCLI権限不足・console認証切れを記録し、本人再ログイン後12:27 JSTのus-east-1照会でinstance／volume／予約IP／snapshot／自己所有AMI／NAT／ENI各0、試験network残存0、専用IAM role／Scheduler group／一時inline policy不存在を確認。残存なしのため追加削除不要、既存別用途資源は変更せず、今回の試験対象の停止・撤去再確認は完了。追加課金試験は開始しない。

**10-27の最新準備:** [新x86_64 binary・専用package・helperと検証記録](../review/2026-10-27-GENERIC-X86-PACKAGE-PREPARATION.md)を固定。ELF／GLIBC最大2.34、九assetの非root DAC・実--help、新helperのMac／native ARM各27件、旧package24件が合格。**x86_64 QEMUのRust回帰は50合格・1失敗・1 ignoreで、GETQDISC／GETTFILTERのerrno95を独立C probeでも再現。全合格へ読み替えない。** 同じRust回帰実行ファイルを同梱し、GCPの最初の非rootservice gateで全51合格を必須化した。新driverはRestart=noを守り、SIGKILL後のlink解放を確認して明示restartする33確認を予定。新しい実XDP・実NIC・GCPは未検証、VM／NIC変更・課金なし、公開HOLD。成果物hash・確定値と未検証範囲は報告から辿る。

**10-27の最新計画（未実行）:** [GCP専用一台の明示Generic試験案](GCP_GENERIC_SINGLE_VM_PLAN_20261027.md)を具体化。LRO／hardware GROを保存状態B0から事前offのB1へ構成し、隔離機能→実NICの通常停止／SIGKILL各一回→B1不変→B0復元→撤去の順。最大60分・US$0.20、T0＋45分のVM／boot disk DELETE、SSHのsource一件/32→TCP22だけ。GCP読取りでVM／disk／予約IP0、image READY・e2-small参照可、管理source111.217.200.148を再確認した。**新binaryは現状ARM64、旧helperは旧hash固定のため、先に非課金でx86_64／package／helperを適合・検証し、hash提示後に課金・NIC変更・削除の実行承認を求める。** 今回は文書化だけでbuild・実装・VM作成・設定変更・課金試験なし。実NIC・公開受入れは未達。

**10-27の最新実装・非課金回帰:** 本人が事前offの専用環境案を採用し、[明示Genericとactive／wanted保全guard](../review/2026-10-27-GENERIC-OFFLOAD-GUARD-IMPLEMENTATION.md)を実装。両modeでon・不明をload前に拒否し、READY前に実mode・所有program・feature等の一致を検査する。Linux非rootのローカルBAN51件、既存node185件、Macのportable10件・node182件、旧package24件が合格。既存binary／object／証拠は保全。**新loaderの実XDP・実NIC・GCPは未検証、設定変更・課金なし、公開HOLD。** 次はNIC事前設定・復旧と新しい実NIC試験の具体案を確認し、別承認後のみ実施する。旧packageを自動更新して再使用しない。

**10-27の最新調査・採否:** [装着経路とVirtIOの条件を一次資料・codeで照合](../review/2026-10-27-GCP-XDP-ATTACH-DESIGN-DECISION.md)。flags0はdriver有無で経路を選び、GenericはFD所有linkでも利用できる一方、装着時にLRO／hardware GROを無効化し、解除ではその設定を復元しません。**保存されたrx-gro-hw:onを維持する一行Generic案は不採用。active／wantedが既にoffの専用環境に限定するGeneric＋保全guardが条件付き候補。** queue不足だけでEINVALを断定できず、交渉済みfeature等がないため実機根因は未確定です。実装・build・機能再試験・GCP API／SSH／container実行なし。過去の証拠を保全し、追加課金は行っていません。次は環境条件の採否判断からで、課金再試験を自動再開しません。

**10-27の最新再試験:** 本人承認後、[GCP一台を一回実行](../review/2026-10-27-GCP-ISOLATED-XDP-PASS-NIC-EINVAL.md)。**非root実起動・隔離実XDPの32確認は合格。実NIC ens4／VirtIO-Net／MTU1460への装着はbpf_link_createのEINVAL（22）で失敗。** mode・MTU・queue・offload・driver・期限・安全条件を変えず、再装着・再作成なし。08:59:44〜09:07:22 JSTの約7分38秒で新設VM／disk／network類を撤去し、既存九区分の選択項目一致と別の不存在照会を確認。今回枠は終了。現行方式の当該実NICへの導入は不可、公開六条件は未達。残りは追加課金なしの装着方式の判断 → 採用する場合だけ別承認の実NIC受入れ → 合格した場合だけ公開受入れです。

**10-27の前段準備:** 追加課金なしで[試験packageの権限・非root事前確認を修正](../review/2026-10-27-GCP-PACKAGE-PREFLIGHT-FIX.md)。24件の回帰に合格し、ローカルLinuxのUID/GID65534・全capability0で旧配置の拒否と新packageの読取り・実行権・書込み拒否を確認。最初のlocal tmpfs noexec失敗も保全。**GCP作成・SSH・設定変更なし。x86_64実起動／systemd／XDPは未検証。** [再試験案](GCP_PACKAGE_RETRY_PLAN_20261027.md)は新しい一台・最大60分・US$0.20・T0＋45分DELETEで、承認前。今回の修正は完了、次は新VMの事前確認と適合ゲート、その後は合格した場合だけ公開六条件です。

**10-27の最新実行:** 本人が一台・最大60分・US$0.20・期限付き削除と撤去を承認し、[GCP試験を一回実行](../review/2026-10-27-GCP-VM-BOOT-AND-EXEC-PERMISSION-FAILURE.md)。VM作成・host key照合・SSH／sudo・隔離HTTPSまでは成功。転送したroot所有binaryが0700で専用非rootユーザーから実行できず、daemonは203/EXECで停止しました。これは試験側の配置権限確認漏れです。**再試験・実NIC装着・BANなし。08:23:06〜08:29:12 JSTで新設VM／disk／network類を撤去、九区分の選択項目一致と別照会の不存在を確認。** 元のcomplete=false／cleanup_ok=falseは保全し、独立停止・退避・撤去の成功と分けました。期限・安全条件・製品codeは変更なし。今回枠は終了し、GCP XDPと公開六条件は未検証。残りは配置属性の修正と事前確認 → 別承認で一台の適合確認 → 合格した場合だけ公開受入れです。

前段の調査時点: 本人の後続指示で、追加課金対象を作らず現行XDPの[四分類対応表](SUPPORTED_ENVIRONMENTS.md)と[GCP一台の具体案](GCP_XDP_SINGLE_VM_PLAN_20261027.md)を作成しました。公式仕様・既存code・保存済み実測と、GCP15件の読取りを照合。project／Compute／billing有効、VM・disk・予約IP0、固定Ubuntu image READY、e2-small／zone／quotaを確認しました。**VM作成要求0件、503解消・GCP guest／NICは未検証。** 案は一台・最大60分、税前見積りUS$0.06・運用上限US$0.20、T0＋45分から自動DELETEと新設物撤去。旧予算枠は流用せず、作成・課金・削除は未承認です。携帯・Pi／hp／routerの操作、実装・設定変更・追加実機試験はありません。詳細・private証拠・残タスクは上記二文書へ記録しました。

前段の採否判断では、**現行BOOTTIME絶対期限を同等に維持する標準nftablesへの移植は不採用**と確定。Linux v7.0でも相対期間による期限延長の反例が成立し、meta time・定期削除・早期失効も同等契約になりません。**nft移植の追加実装・期限試験は終了し、owner成功・旧失敗・試作は保全。** [採否根拠と残タスク](../review/2026-10-27-NFT-BOOTTIME-ADOPTION-DECISION.md)。今回のGCP案は現行XDPの適合確認であり、nft候補の再開ではありません。公開pilotは引き続き未達です。

前段のowner確認では、本人の実行承認後、hp-ubuntuの新規PrivateNetwork unitで一回実施しました。**所有process生存中のsocket closeと、別の子のSIGKILLの両方で、所有tableだけを回収・対照table存続を実確認。最後はtable0、unit停止・非enabled、専用cgroupとlsns上のprivate namespaceも不存在です。** 実権限はCAP_NET_ADMINだけ、NoNewPrivs=1、loのみ。時間上限は維持し、再試験なし。新規SSHと既存状態の一致を確認、証拠7ファイルをhash照合して退避しました。**owner限定ゲートは達成。BAN適用の採否は後続で別判定しました。** [実試験・回収・保全](../review/2026-10-27-NFT-OWNER-ISOLATED-ACCEPTANCE.md)。

前段の最小修正では、probeのPID 1直接参照を固定のroot所有事前参照へ置き換え、root0700の親・root0400の通常ファイル・単一リンク・4KiB上限・schema・同じboot・読取り中変更を検証しました。既存の隔離・権限・時間条件を維持し、既存15件を含む24件のoffline検証が成功。[修正前後の反例と証拠](../review/2026-10-27-NFT-OWNER-HOST-REFERENCE.md)。この修正段階ではhpへの配置・隔離実行は行っていません。

前段のhp-ubuntu読取りでは、新規SSH／sudo、既存tool、nf_tables、未使用の試験対象を確認しました。配置・unit・namespace・tableは未作成。PID 1 namespaceへの通常ユーザーからの参照拒否を確認し、最小権限probeの直接参照を修正対象としました。link・nft・service名は前後一致、address／routeの差は有効残時間だけです。[読取り結果と失敗記録](../review/2026-10-27-NFT-OWNER-READONLY-PREFLIGHT.md)。

前段では、本人が現行のBOOTTIME絶対期限維持とBAN適用保留を選択。空のowner tableだけの[隔離確認案](NFT_OWNER_PROBE_PLAN.md)・専用script・復旧手順を準備し、15件のoffline検証を確認しました。socket closeは子processが生存中に観測し、SIGKILLは別caseとします。**hpの新規PrivateNetwork unitで最大60秒・空table最大2個という案で、実機は未実行。chain・rule・set・BANなし。** [準備結果・未検証・終了条件](../review/2026-10-27-NFT-OWNER-PROBE-PREPARATION.md)。この準備段階ではcloud／hp／Piへ接続していません。

その前段ではnftables候補の権限分離・停止復旧を独立Rustモデルで設計し、27件のRust検証、Clippy、format、apply拒否、既存製品hash保全を確認しました。相対TTLへの単純移植による絶対期限延長を反例化し、今回も契約を変えていません。**実nftables・OS権限・packet・SIGKILLは未検証、BAN実適用はBLOCKED。①②全体・③の完了とは扱いません。** [モデルの結果と証拠](../review/2026-10-27-NFT-BOUNDARY-OFFLINE.md)・[設計契約](NFT_BOUNDARY_DESIGN.md)。以下は保全した実試験の履歴です。

**2026-10-26までの到達点は、隔離Linuxの実XDPとhp-ubuntuの物理NICでの一件LAN試験まで確認済み。公開WANでのBAN・解除は未実施で、公開サイトの試験導入はHOLDです。GCPは承認Aと別zoneへの承認済み一回比較がともにVM作成HTTP503で停止し、新設network等の撤去・保全照合まで完了。GCPの実XDP試験は未実施です。携帯テザリングは以後使わず、自宅追加WAN試験は保留です。**

後続のAWS一台試験では、**VM作成・SSH・隔離実XDPの32確認は成功。実仮想NICのENAは既定MTU9001を拒否し、kernelが上限3498と明示したため、設定を変えず停止・撤去しました。** 一台の実行枠は21:45:53〜21:52:01 JST、別照会でも新設物の不存在を確認。公開受入れは未達です。[AWS実試験・MTU原因・撤去と次の最小案](../review/2026-10-26-AWS-SINGLE-VM-XDP-AND-MTU.md)。現在の残りは、対応NIC条件での適合確認 → 別承認による公開六条件 → 撤去・操作確認・限定導入判定です。

**22:26 JSTの停止状況再照合:** 読み取りだけでGCPの`veil-x-tests`を再確認し、VM・disk・予約address0、残るnetwork／subnet／firewallはdefault系でした。AWSのus-east-1も稼働instance・volume・予約address0、今回instanceはterminated、試験tagのVPC／subnet／SG／route table／IGW0、専用role・Scheduler group・一時読み取りpolicy不存在を再確認。新規作成・設定変更・追加課金試験は行わず、CloudShellをexitし今回の作業タブを閉じました。[GCP等のCLI結果](../../artifacts/cloud-stop-audit-19nxscjc/cli-summary.json)・[AWSの再照合](../../artifacts/cloud-stop-audit-19nxscjc/aws-summary.json)。全account／全region／全serviceの停止や確定請求額0を主張する確認ではありません。本人の「不確実性を確定するためなら課金可」は、既存枠の延長や新規resource作成の包括承認には読み替えず、次回の仮説・対象・上限・撤去を具体化して判断します。

## 記録の読み方

最新の[503要求・監査ログ分析](../review/2026-10-26-GCP-503-REQUEST-ANALYSIS.md)では、三要求のcode14、七権限granted、boot一枚・固定image・削除期限の整合を確認しました。明白な要求ミスは見つからず、根本原因は未確定。追加作成なし、公開Bは保留です。

その記録を基に[問い合わせ文を準備](../review/2026-10-26-GCP-503-SUPPORT-DRAFT.md)。後続の本人承認で[公式窓口からの送信に着手](../review/2026-10-26-GCP-SUPPORT-CHANNEL-HOLD.md)しましたが、契約はベーシック（お支払い関連のみ）で、作成フォームは空・ケース一覧はサービスエラーとなり、**受付未成立・未送信**です。Googleの回答待ちではありません。有料契約・公開投稿・追加作成・公開Bは行っていません。

**GCP停止時点の工程は、VM準備の障害解消 → GCP隔離実XDP → 承認B後の限定公開受入れ → 撤去・導入判定でした。** 環境正常・承認準備済みなら実試験〜撤去は工程見積り2〜4時間ですが、503解消待ちは含まず未実測です。後続のAWS結果と現在の残りは冒頭に追記し、元の時間・予算枠を延長しません。今後は残り・次の一手・終了条件を短く併記します。

後続の[公式eBPF／XDP対応確認](../review/2026-10-26-GCP-EBPF-XDP-OFFICIAL-FEASIBILITY.md)では、Cloud Runは非対応、今回選んだCompute Engineはゲスト管理権限と公式driverのXDP対応に基づく条件付き候補と整理しました。kernel／NIC／modeは未検証で、作成503とは別の論点です。追加作成・課金・設定変更なし。次回は一台の適合確認を先に終える順序を推奨します。

本人から一台の適合確認を先に行う指示を受領し、[一台・一回・最大60分の再開案](GCP_SINGLE_VM_COMPATIBILITY_PLAN.md)を準備。後続の「推奨で」を具体案への承認として実行しましたが、**Ubuntu 24.04でも一回のVM insertが503で失敗。再送せず、20:05:54 JSTに新設物の撤去と九区分の選択項目一致まで完了**しました。guest SSH・GCP実XDPは未実施。[一台試験の失敗・監査・撤去結果](../review/2026-10-26-GCP-SINGLE-VM-CREATE-503.md)。元の終了済み枠を延長したのではなく、別の承認済みUS$0.20枠で実施した一回です。

後続の[請求・CLI・公式仕様確認](../review/2026-10-26-CLOUD-BILLING-STATUS-AND-AWS-CANDIDATE.md)では、請求有効・請求先open・project ACTIVE、CLI到達性診断の合格を確認。カード会社の内部制限は未確認です。GCPの追加作成は保留し、本人から出たAWS EC2案について公式のENA／XDP資料を確認しました。**既存AWS accountの有無を確認中、AWSへの認証・作成・課金は未実施**。CloudflareのWAFによるサイト防御と、今回のLinux用XDP製品の実証を区別します。

その後、既存AWS accountを確認。rootの既存MFAは本人のログインで使用でき、承認済みのバージニア限定EC2読み取り権限一件を一時追加してCLI九照会に成功しました。**追加した権限は撤去し、元の二ポリシーとEC2拒否への復旧を確認。VM作成・課金試験は未実施**です。[MFA・CLIの結果と、一台試験の作成前案](../review/2026-10-26-AWS-MFA-CLI-READONLY-AND-SINGLE-PLAN.md)。

- **確認済み**は、リンク先に記録した成果物・環境・条件での結果です。別binary、別NIC、公開経路へ自動的に転用しません。
- **失敗・中止**は、その停止段階を明示します。復旧が成功しても試験の合格へ書き換えません。
- **机上・オフライン**は、保存出力や模擬環境の検証です。実通信・実運用者の操作・実機復旧とは区別します。
- **計画・未実施**は、承認や準備があっても実行・成功を意味しません。
- 各報告は記載時点の作業ツリーの記録です。公開Releaseや現在のmain CIの成功を示しません。後続報告で解消した点も、以前の失敗を残したままリンクで辿ります。

## 1. BFTの安全性修正と保存待ちの調査（保存した実験履歴）

現在のローカル一時BANとは別経路です。旧WAL・鍵・反例・実験記録を保全し、ローカル一時BANへ自動移行していません。

| 日付 | 何を調べたか・試したか | 結果・判断と詳細記録 |
| --- | --- | --- |
| 10-23 | QC拒否後の状態変更、永続化前の投票応答などの重大欠陥 | 具体的な失敗経路を修正・回帰確認。本番投入完了や完全なBFT証明とは扱わない。[初回レビュー](../review/2026-10-23-V091-IMPLEMENTATION-REVIEW.md)・[修正記録](../review/2026-10-23-CRITICAL-FIXES.md) |
| 10-23〜24 | TTL・共通期限・署名のクラスタ／構成binding・固定membership | 対応した反例と互換性条件を個別に記録。[TTLと復旧](../review/2026-10-23-TTL-BYPASS-RECOVERY.md)・[共通期限](../review/2026-10-23-SIGNED-DEADLINE-CATCHUP.md)・[クラスタbinding](../review/2026-10-23-CLUSTER-CONTEXT-BINDING.md)・[構成digest](../review/2026-10-24-FIXED-MEMBERSHIP-DIGEST.md) |
| 10-24 | 通常daemon、ノード別TLS、欠損WALの拒否、有限TTL必須化 | 操作契約と隔離検証を記録。[daemon](../review/2026-10-24-DAEMON-OPERATION-FLOW.md)・[TLS](../review/2026-10-24-NODE-TLS-OPERATIONS.md)・[WAL起動](../review/2026-10-24-WAL-BOOTSTRAP-RECOVERY.md)・[有限TTL](../review/2026-10-24-FINITE-BAN-POLICY.md) |
| 10-24 | 高term・heartbeat再送、検閲リーダー、競合する未確定prefix | prepare／commitを分離し、証拠付き交代へ変更。旧WALを自動移行しない。[termとfreshness](../review/2026-10-24-TERM-ADMISSION-AND-FRESHNESS.md)・[検閲対処](../review/2026-10-24-WITNESSED-BAN-PROGRESS.md)・[二段階合意](../review/2026-10-24-TWO-PHASE-VIEW-CHANGE.md) |
| 10-24 | WAL増大、checkpoint・状態転送、BAN権限と範囲 | 容量・復旧・承認の契約を記録。[checkpoint](../review/2026-10-24-BOUNDED-WAL-CHECKPOINTS.md)・[運用者承認](../review/2026-10-24-SCOPED-OPERATOR-APPROVAL.md) |
| 10-24 | 時計品質、TLS更新後の待ち、保存経路の区間計測、sync_dataとの比較 | ファイル同期だけでなく親directory同期・状態ロック待ちを分離。sync_data単独への置換で解決したとは判定しない。[時計とTLS](../review/2026-10-24-CLOCK-QUALITY-AND-TLS-LATENCY.md)・[区間計測と比較](../review/2026-10-24-WAL-SYNC-COMPARISON.md) |
| 10-24 | 検閲交代の停止段階、PrepareからCommit保存まで、必須fsync内の待ち | 単独・並行時の失敗を切り分け、同期・TTL・安全条件を固定して調査。[並行時の停止段階](../review/2026-10-24-PARALLEL-CENSORSHIP-RECOVERY.md)・[保存待ち](../review/2026-10-24-PREPARE-COMMIT-WAIT.md)・[ジャーナル待ちの境界](../review/2026-10-24-JOURNAL-WAIT-BOUNDARY.md) |
| 10-24 | 完全snapshot追記の独立試作、独立host比較、現方式の採否 | 試作の成功を本体採用と同一視せず、BFTの公開導入は保留。[独立試作](../review/2026-10-24-SNAPSHOT-APPEND-PROTOTYPE.md)・[host比較](../review/2026-10-24-INDEPENDENT-HOST-COMPARISON.md)・[後続daemon比較](../review/2026-10-24-DAEMON-HOST-ACCEPTANCE.md)・[導入判断](../review/2026-10-24-PILOT-DECISION-AND-TARGET-DESIGN.md) |

## 2. ローカル一時BANと隔離環境での復旧

公開サイト向けの最小構成を再検討し、ユーザーが「再起動で一時BANが消える」契約を採用しました。BFTの投票前永続化を省略したのではなく、別binary `veil-local-ban`として合意・WALを必要としない用途へ絞っています。[設計時の判断と限界](../review/2026-10-24-PUBLIC-SITE-BOUNDED-MINIMUM.md)を参照してください。

| 日付 | 試験・対象 | 結果と残した境界 |
| --- | --- | --- |
| 10-24 | pi-node隔離Linux、ローカル一時BANの最小実装 | 正常通信・遮断・解除・期限・256件上限・再起動時消失を確認。[実装と受入れ](../review/2026-10-24-LOCAL-EPHEMERAL-BAN-IMPLEMENTATION.md) |
| 10-24 | SIGKILL後の管理socket回収、初期化中断、所有記録破損 | 所有を確認できる場合の再起動と、確認できない場合の保全・起動拒否・固定別directoryへの手動復旧を確認。[SIGKILL](../review/2026-10-24-LOCAL-BAN-CRASH-RESTART.md)・[手動復旧](../review/2026-10-24-LOCAL-BAN-MANUAL-RECOVERY.md) |
| 10-24〜25 | systemd、専用UID、常設unit、修正版scriptの通し実行 | 隔離環境で起動・停止・再起動・手動復旧を確認。試験unitは停止・disabled。初回の分割実行と後の通し成功を分けて保全。[運用](../review/2026-10-24-LOCAL-BAN-SYSTEMD-OPERATIONS.md)・[常設受入れ](../review/2026-10-25-LOCAL-BAN-PERMANENT-SYSTEMD-ACCEPTANCE.md)・[通し結果](../review/2026-10-25-LOCAL-BAN-FULL-RUN-ACCEPTANCE.md) |
| 10-25 | pi-nodeの正常host再起動、namespace復元と自動起動 | 新instance／BAN0を確認。ただしmainnet同一container ID監査は未達、再buildも当時失敗。全体成功にはしない。[bootの結果と失敗](../review/2026-10-25-LOCAL-BAN-BOOT-ACCEPTANCE.md) |
| 10-25 | 再起動後のNixパス不在、再buildと回帰、その成果物の実XDP | 既存データの所在を確認し、build専用sessionで復旧。新binaryで通信・容量・期限・解除・restartを確認。host全体のNix常設復旧ではない。[build復旧](../review/2026-10-25-NIX-BUILD-RECOVERY.md)・[新成果物のkernel受入れ](../review/2026-10-25-REBUILT-LOCAL-BAN-KERNEL-ACCEPTANCE.md) |
| 10-25 | 一覧・preview・明示適用／解除、古いinstance／revision、pi-node実XDP | 新しい操作支援付きbinaryで拒否・容量境界・期限・restartを確認。以前のbinaryのboot／破損試験を全て再実行したとは扱わない。[実XDP受入れ](../review/2026-10-25-LOCAL-BAN-OPERATOR-KERNEL-ACCEPTANCE.md) |
| 10-25 | 保存済み出力による本人の机上確認、6場面 | 一覧・preview・実通信確認の必要性、競合・古いinstance・結果不明時の判断を言語化。結果不明caseは想定問題。無援助の実機操作・所要時間は未測定。[回答と未検証事項](OPERATIONS.md#7-机上確認の記録2026-10-25) |

## 3. hp-ubuntuの実XDPとLANの物理NIC

| 日付 | 試験・対象 | 結果と終了状態 |
| --- | --- | --- |
| 10-25 | 新Ubuntu host、隔離namespace／veth、同一hashのbinaryとobject | 正常HTTPS・遮断・解除・期限失効・256／257件・通常／SIGKILL restartを確認。終了後は停止・XDP解除・既存環境の前後比較を確認。物理NICの試験とは別。[隔離受入れ](../review/2026-10-25-HP-UBUNTU-ISOLATED-XDP-ACCEPTANCE.md) |
| 10-25 | 物理eno1／e1000e、Generic XDP、LANの一件BAN | pi-nodeを試験clientとして一件TTL30秒BANし、対象だけの遮断・期限前の明示解除・Mac正常client・新規SSH・対照port継続を確認。所有XDP解除と停止・非enabledで終了。終了監査の自己観測誤検出は原記録を残して独立監査で確認。[計画と復旧](../review/2026-10-25-HP-UBUNTU-PHYSICAL-NIC-PLAN.md)・[LAN受入れ](../review/2026-10-25-HP-UBUNTU-PHYSICAL-NIC-ACCEPTANCE.md) |
| 10-26 | LANリンク喪失後の確認 | 写真のNO-CARRIERと、本人のケーブル挿し直し後のリンク・SSH復帰を記録。前日の最初の切断原因まで断定しない。[復帰とWAN予備試験](../review/2026-10-26-WAN-SOURCE-PREFLIGHT.md) |

物理LANの一件試験は、公開WAN、native XDP性能、他NIC、長期運用の成功ではありません。容量・自然失効・restartの障害注入は隔離受入れの記録であり、同じ試験を物理NICで全て行ったとは扱いません。

## 4. 自宅WAN試験：到達未確認・開始前中止・手順修正

送信元一件・TCP18443、BANを行う場合のTTL30秒、daemon180秒／HTTPS240秒の制限と、Pi・既存転送・基礎防御の保全を維持しました。以下の中止をWANでのBAN失敗と混同しません。動的な送信元IPや認証情報をこの索引へ転記せず、過去のIP・承認を次回に再利用しません。

| 日付 | 実施したこと | 停止段階・結果・復旧 |
| --- | --- | --- |
| 10-26 | ケーブル復帰後、携帯回線から送信元限定WAN予備試験 | LANのHTTPSと送信元制限は確認したが、携帯側TCP到達は確認できず、XDP前に停止。追加診断も保存前のsource不一致で未実施。転送off・試験停止・Wi-Fi復元・管理疎通を確認。[予備試験](../review/2026-10-26-WAN-SOURCE-PREFLIGHT.md) |
| 10-26 | Macの送信元TCP port固定とTLS接続保持、外向きIP測定 | 二宛先への保持接続、約50秒・12応答で同一外向きIPv4を観測。hp向けNAT mappingの保証ではない。Pi・hp・routerは操作せず、Wi-Fi復元。[測定](../review/2026-10-26-MOBILE-FIXED-SOURCE-PORT-MEASUREMENT.md) |
| 10-26 | 保持接続を使う限定WAN試験の準備と追加試行 | 照会枠不足・保存承認の有効時間切れでWAN接続前に中止。XDP・BAN・解除未実施。転送off・サービス停止・Wi-Fi復元・保全を確認。[中止の内訳](../review/2026-10-26-PINNED-WAN-TRIAL-HOLD.md) |
| 10-26 | 承認待ちと実試験の時間枠を分離したhelper | 模擬時計・通信等のオフライン28項目を確認。この段階ではnetwork変更も実WAN試験もなし。[変更案](../review/2026-10-26-WAN-APPROVAL-AND-TRIAL-PLAN.md)・[当時の検証](../review/2026-10-26-WAN-PHASES-OFFLINE-VERIFICATION.md) |
| 10-26 | 改修手順での実機予備試験 | 承認待ちを分離できたが、保存時のrouter認証切れで中止。保存結果不明のため再クリックせず、WAN接続・SYN観測・XDP・BANへ進まなかった。再ログイン後に転送off、停止・Wi-Fi復元・保全を確認。[実機記録](../review/2026-10-26-WAN-PHASED-LIVE-PREFLIGHT.md) |
| 10-26 | 復旧helperのstdin契約漏れとWi-Fi復元の読取り判定 | 原失敗を保全した別コピーで修正し、オフライン30項目を確認。修正版を再配置した実機再試験は未実施。試験未完了と、独立確認による復旧完了を分けて記録。[修正と証拠](../review/2026-10-26-WAN-PHASED-LIVE-PREFLIGHT.md#最小修正と検証) |

この段階の障害を、製品のXDP・BANロジックやBFTの遅延が原因と決めつけません。公開経路の未到達と、試験前の認証・手順不備をそれぞれ残しています。その後[自宅追加試行は最大一回・30分の上限](DELIVERY_GATE.md#試験の順序と上限)を合意しましたが、最新の本人指示により携帯テザリングは使わず、追加WAN試験は保留します。上限到達も、保留も、合格とはしません。

## 5. クラウド試験：作成失敗と、その後の一台起動

2026-10-26、当初の[DigitalOcean案](DIGITALOCEAN_TRIAL_PLAN.md)から、本人が既に使うGCPの`veil-x-tests`へ変更しました。DigitalOceanは比較・計画のみで、新規VM試験の結果はありません。

- **実施:** GCP browserでproject IDを確認。VM一覧はCompute Engine API有効化画面へ進みました。概要の「0」をresource不存在の証明にはしていません。CLI認証は失敗し、更新していません。
- **後続の実施:** 規約表示を踏まえた本人承認後、Compute Engine APIを一度有効化し、詳細の「有効」を確認。既定accountのEditor、default networkと四rule、VM一覧空、地域CPU／外部IPv4割当、請求先リンク、未保存フォームの候補見積りを確認しました。[有効化後の記録](../review/2026-10-26-GCP-API-ENABLED.md)から、[有効化前の停止記録](../review/2026-10-26-GCP-API-PREFLIGHT.md)も辿れます。
- **未実施:** VM作成、課金設定変更、手動のIAM／network変更、公開、XDP装着、クラウド通信試験、削除。API有効化時のGoogle側生成物と、こちらの手動resource作成を区別します。
- **構成案:** 最大二台・最大四時間・US$1上限案。作成前に見積り・許可範囲・管理経路・復旧／撤去対象を確定します。時間・費用の案を作成承認とは扱いません。
- **後続の設計（10-26）:** [二台構成・作成前確認案](GCP_TWO_VM_EXECUTION_PLAN.md)を作成。Macの有線sourceを再確認し、Cloudflareで既存`api.veil-s.com`を保全対象とし、未使用の専用FQDN候補を確認しました。DNS-01による公開証明書、二つの/32限定・TCP18443、税前計画額$0.22とUS$1の非強制上限、期限付き新設VM／disk削除の副作用と手順を整理。**計画・読取り確認のみで、VM・鍵・DNS・証明書・ruleの作成はなし。**
- **作成A受領・準備（10-26）:** 指定対象の作成・課金・期限付き削除・撤去の承認を受領しました。CLIの読取りで`invalid_grant`となり、本人の再認証待ち。既存と同一hashの成果物・隔離driverを別のprivate領域へ保全し、host namespace等で開始しない四つの模擬拒否経路を確認。**VM作成なし、四時間枠は未開始、Bは未承認。** 追加のUS$100は今回のUS$1枠の増額とせず、費用対効果の再判断線として記録。[準備結果と再開位置](../review/2026-10-26-GCP-A-PREPARATION.md)
- **作成Aの実行・撤去（10-26）:** 再認証後、14:46:50 JSTにT0を開始し、専用VPC／subnet／限定SSH ruleを作成。target VM挿入POSTが503で失敗。残存なしの確認後に一回だけ同じ構成・期限で再試行したが503で、client VMは未要求。所有ID照合で新設物を撤去し、14:58:32に9区分がbaselineと一致しました。**二台準備・GCP実XDPは未達、撤去は完了、Bは未実施。** 実請求額は未照合。[失敗・再試行・復旧の詳細](../review/2026-10-26-GCP-A-VM-CREATE-503.md)
- **zone b一回比較・撤去（10-26）:** 本人承認後、原T0・US$1・絶対期限を維持して`us-central1-b`へ変更。15:15:03 JSTのtarget作成一回もPOST503で失敗し、client未要求・再送なし。新設SSH rule／subnet／VPCを今回の所有IDで撤去し、15:17:00に九区分が比較前と最初のA開始前の両方に一致。**zone変更だけでは解消せず、GCP実XDPは未着手、公開Bは保留。** [一回比較・停止・復旧の詳細](../review/2026-10-26-GCP-A-ZONE-B-COMPARISON.md)
- **503読取り分析（10-26）:** 保存要求・SDK・既存監査ログを照合。三件ともGCPでcode14、記録された七権限granted、boot一枚・固定image・削除期限は整合。Compute service agent roleも存在。**根本原因未確定、明白な要求ミスなし。** 期限・Shielded・IAMを弱めず、最小案は限定監査情報の相関保存。追加作成・設定変更なし、15:30:22の再確認でVM0・disk0。[候補・最小変更案・残り四工程](../review/2026-10-26-GCP-503-REQUEST-ANALYSIS.md)
- **問い合わせ文準備（10-26）:** 保存した三件のUTC時刻・zone・監査insertIdを照合し、内部reason、構成の既知問題、安全条件を維持した最小対処を尋ねる[日本語の完成稿を作成](../review/2026-10-26-GCP-503-SUPPORT-DRAFT.md)。**本人確認待ち・未送信、添付なし。** 監査IDをCompute requestIdと混同しない。追加作成・新たなGCP API照会・設定変更・公開Bなし。
- **送信承認後の窓口確認（10-26）:** 本人承認を受領して公式の認証済みSupportへ進み、ベーシック（お支払い関連のみ）を確認。ケース作成フォームは空・ケース一覧はサービスエラーで、**受付未成立・未送信、ケース番号なし**。本文・監査ID入力、添付、有料契約、公開forum投稿、追加VM作成、公開Bはなし。証拠を保全し作業用タブを閉じました。契約範囲と画面エラーの因果は未確定。[停止理由と再開条件](../review/2026-10-26-GCP-SUPPORT-CHANNEL-HOLD.md)
- **公式の低レイヤー対応確認（10-26）:** Cloud RunのeBPF非対応と、Compute EngineのゲストOS管理・Google公式gveのXDP対応条件を一次資料で確認。**Compute Engineは条件付き候補、現VM構成での製品対応は未検証**。既存loaderのmode指定・FD所有条件と、物理host／NICを操作する試験ではない境界も記録。ゲストログはないが監査ログは取得済み。作成503の原因は未解消で、追加作成・課金・製品変更なし。[公式根拠・限界・一台からの確認順序](../review/2026-10-26-GCP-EBPF-XDP-OFFICIAL-FEASIBILITY.md)
- **一台確認の準備（10-26）:** 本人の方針指示後、九区分と管理元を読み取り、Ubuntu 24.04の固定imageを確認。一回のimage変更比較として、一台・最大60分・追加US$0.20、T0＋45分autoDELETEと新設物撤去を含む[具体案を作成](GCP_SINGLE_VM_COMPATIBILITY_PLAN.md)。作成・課金・削除・公開Bは未実施。イメージ照会のCLI構文誤りと修正後の読取り結果も保全。
- **一台確認の実行・撤去（10-26）:** 具体案の承認後、20:04:28 JSTに新しいT0を開始し、専用VPC／subnet／限定SSH ruleを作成。Ubuntu 24.04を指定したVM insert一回もHTTP503、監査code14で失敗。追加送信なし。20:05:54 JSTに所有ID照合による新設物撤去と、九区分の選択項目一致を確認しました。**VM0・disk0、guest SSH・XDP・公開Bは未実施。** イメージ変更だけでは解消せず、実適合性は未判定。実請求は未照合。[監査ログと撤去の結果](../review/2026-10-26-GCP-SINGLE-VM-CREATE-503.md)
- **請求・CLI確認とAWS候補（10-26）:** 20:47 JSTにbillingEnabled=true、請求先open=true、project ACTIVE、Compute有効、VM0・disk0を確認。CLIの到達性診断も合格。公開statusは広範囲の重大障害なしで、個別の作成障害までは否定しません。AWS EC2のENA／XDPと有効化待ち等のerror codeを公式資料で確認し、既存accountの有無を質問。**追加作成・支払い・設定変更なし、AWS API照会なし**。[確認範囲・限界・次の判断](../review/2026-10-26-CLOUD-BILLING-STATUS-AND-AWS-CANDIDATE.md)
- **AWS MFA・CLI読み取り（10-26）:** 既存accountと本人が使用できるroot MFAを確認。CLI認証は有効だがEC2権限不足だったため、保存直前の本人承認後、us-east-1のDescribe九項目だけを一時追加し、EC2／EBS0と公式AMI・t3.smallを照会。内容照合して追加一件だけを撤去し、21:25 JSTに元の二ポリシーとCLI拒否への復旧を確認。**新規MFA・キー・VM・課金試験なし。** 一台・一回・US$0.20／60分と終了・撤去の案を記録。[実施結果と作成前案](../review/2026-10-26-AWS-MFA-CLI-READONLY-AND-SINGLE-PLAN.md)
- **AWS一台の実XDP・ENA条件（10-26）:** 具体案の本人承認後、RunInstances一回で作成・SSH成功。既存と同一hashの製品／object／driverで隔離実XDPの32確認が成功。ens5／ENAの装着は`bpf_link_create`のEINVALで失敗し、同時刻のkernelがMTU9001に対し上限3498と明示しました。MTU・modeを変更せず停止、新規SSHと非装着・route／TC保全を確認。21:52 JSTに新VM／disk／network／Scheduler／roleを撤去し、独立照会でも確認。**公開B・二台目・実NIC装着中のSSHは未検証、導入HOLD。** [結果・原出力・最小の次案](../review/2026-10-26-AWS-SINGLE-VM-XDP-AND-MTU.md)
- **保全:** APIと既定生成物を残し、未保存フォームからVMを作成していません。Pi・hp・routerの設定変更やMacのWi-Fi変更なし。本人指示により携帯テザリングは以後使わず、自宅WANを再開しません。GCP作業用タブは終了時に閉じます。

詳細は[GCP試験計画](GCP_TRIAL_PLAN.md)。実施後は日付、実際のimage／kernel／NIC、成果物hash、case別結果、未確認、停止・復旧・課金対象の残存を持つ別報告を追加し、この索引からリンクします。

## 6. GCP一台起動と、配置権限による停止

| 日付 | 目的 | 結果と証拠 |
| --- | --- | --- |
| 10-27 | e2-small／VirtIO-Net／MTU1460で一回の適合確認 | VM作成・SSH／sudo・隔離HTTPSは成功。root所有binary0700で製品daemonのexecに失敗。権限変更して再試験せず、BAN・実NICは未実施。約6分6秒で撤去・既存九区分一致・別照会の不存在を確認。[失敗原因・停止と撤去](../review/2026-10-27-GCP-VM-BOOT-AND-EXEC-PERMISSION-FAILURE.md) |

## 7. 配置属性と非rootゲートの修正（追加課金なし）

| 日付 | 目的 | 結果と証拠 |
| --- | --- | --- |
| 10-27 | 700／600の再発防止と非root事前確認 | packageの属性固定・拒否検証・実アクセス確認を追加。Python24件、ローカルLinuxの実非root読取り／実行権・書込み拒否を確認。systemd／x86_64起動／XDPは未検証。[修正と検証](../review/2026-10-27-GCP-PACKAGE-PREFLIGHT-FIX.md)・[次回の新規承認前案](GCP_PACKAGE_RETRY_PLAN_20261027.md) |

## 8. GCP再試験：隔離合格と実NICの拒否

| 日付 | 目的 | 結果と証拠 |
| --- | --- | --- |
| 10-27 | 修正版packageで非root実起動から実NICまで一回判定 | 実--help・隔離32確認・終了復旧は合格。実NIC装着はEINVALで停止。mode等を変えず約7分38秒で撤去し、既存九区分一致と別照会の不存在を確認。[合格範囲・拒否箇所・撤去と残り](../review/2026-10-27-GCP-ISOLATED-XDP-PASS-NIC-EINVAL.md) |

## 9. EINVAL・VirtIO・Generic副作用の採否（調査のみ）

| 日付 | 目的 | 結果と証拠 |
| --- | --- | --- |
| 10-27 | 実装・再試験前に最小適合案を判定 | kernel／Ayaと保存済み状態を照合。Genericのoffload変更と解除後非復元を確認し、on状態保全の一行変更は不採用。事前offの専用環境に限るguard付き案を条件付き候補とした。実機根因は未確定、実装・再試験・課金なし。[根拠・条件・残り](../review/2026-10-27-GCP-XDP-ATTACH-DESIGN-DECISION.md) |

## 現在残る受入れ条件

公開経路での正常HTTPSと実peer、一件BANと同じ入口の独立正常client継続、期限前の明示解除、元のTTLによる自然失効、通常daemon restartと旧操作拒否・停止復帰、本人の実機操作・復旧を、採用する構成で確認する必要があります。

[README](../../README.md) → この履歴 → 個別報告 → 報告内の証拠、の順で辿れます。`artifacts/`への証拠リンクはローカル保全物を含み、公開repositoryに同梱されるとは限りません。証拠が手元にない場合は報告書だけで原出力を再確認したと扱わず、必要な非秘密の証拠を別途用意します。

継続時は、既存の成功・失敗を書き換えずに新しい行と詳細報告を追加します。承認回答や環境復旧だけを、新しい試験成功として加算しません。
