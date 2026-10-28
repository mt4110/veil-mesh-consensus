# 新x86_64版・専用package・helperの準備結果

2026-10-27 JST。gpt-6.1-sol／high、分担なし。最終ゴールは小規模公開サイトへの安全な限定導入。現在のマイルストーンはGCP専用VirtIO NICの明示Generic適合。今回は[一台案](../local-ban/GCP_GENERIC_SINGLE_VM_PLAN_20261027.md)の課金前準備として、成果物と検証・実行手順を固定した。

**渡せる成果物は、検証用x86_64 packageと、そのsource／toolchain／全asset hash／回帰・失敗記録・操作手順の一組。製品の公開ReleaseやGCP適合合格ではない。** VM作成・NIC設定変更・XDP装着・GCP課金は0件。本人の追加の課金意向は受領したが、今回は確定値を提示する原依頼の範囲までとした。作成・NIC変更の保留は維持する。

## 1. 固定した成果物

[最終package](../../artifacts/generic-package-_y8f579p/generic-trial-final.tar.gz)は16,367,020 bytes。manifest以外の九assetは展開時計64,543,807 bytesで、検証器の64MiB上限内。root所有、binary／native-regression0755、他0644、単一link、固定memberのみ。旧archiveと中間candidateも保全した。

| 対象 | SHA-256 |
| --- | --- |
| 新x86_64 veil-local-ban | 8944adb24c33cbe68a59ca077458b374859a49b8cc09f067f0142b848b70c90b |
| 同梱native-regression | c6ceb21a8a52260a9382e907b3a7397df76dc3ee10790df0114fce790f95aef1 |
| 同一BPF object | 2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f |
| 最終archive | 3b1a9c61c0b09c079a14c0431a34e45a023446e500fdb8c4962a91917ddf401a |
| 最終manifest | 50745315890c4770a3ae06bf21b48b1c715e6bd15043aaa435762ec918466a35 |
| 外部catalog | 76c311568f3ced578606eb4d7512487bc277797fa9657fdf7cbceef2b6e5a735 |

[全memberのhash・mode・size](../../artifacts/generic-package-_y8f579p/package-final-result.json)と[外部catalog](../../artifacts/generic-package-_y8f579p/catalog-final.json)を分離して保存。manifestの自己整合だけで改竄対策を済ませず、外部catalog SHAへ全assetを固定する。再build時のdebug path／gzip headerまでbit単位で同一になると主張する成果物ではない。

## 2. buildと保全

既存Colimaにはqemu-x86_64が登録済みだったため、その設定を変えず使用。公式Rust 1.98-bookwormのAMD64 imageを[Docker Hubのmetadata](https://hub.docker.com/v2/repositories/library/rust/tags/1.98-bookworm)で確認し、manifest digest c49256cbe5ea0188bc658a689500d70c41eb51f009a7a7be209caf60a944f3ecへ固定して追加取得した。約563MBの取得で、旧imageの置換・削除はしていない。

Linux ARM64 kernel6.8.0-90上のQEMU x86_64、Rust1.98.1、Debian bookworm／glibc2.36。UID／GID65534、cap-drop=ALL、NoNewPrivs、network=none、read-only rootfsで、sourceと公開crate registryだけを共有。新workspace／targetでoffline・locked buildした。VMでのbuildや、既存hp／Piへの移転はない。

製品はx86-64 ELF、interpreterは/lib64/ld-linux-x86-64.so.2、要求GLIBCは最大2.34と確認。Ubuntu24.04のglibc2.39を超える要求はないが、実GCPでの実行成功を保証する結果ではない。製品binaryはdebug版のまま。テスト実行ファイルは別copyのdebug／symbol tableをstripし、元の非strip版を保持、52 testsの列挙が残ることも確認した。テスト本体・期待値・無視指定は変更していない。

製品Rust source・Cargo.lock・BPF objectは変更なし。旧binary・archive・driver・旧package／access helper等10対象の[hash一致](../../artifacts/generic-package-_y8f579p/preservation.json)を確認。新たなhelperは[scripts/generic-trial](../../scripts/generic-trial/README.md)へ独立配置した。秘密鍵・token・認証directory・Docker socketはbuild containerやpackageへ渡していない。

## 3. helperの安全境界

- feature GETは最大256件・固定長名・activeとwanted・available／never_changedを検証する。SETを実行しない読取りhelperと、純粋なoff／restore引数計画を分けた。
- NIC orchestratorはroot所有のpackage／manifest、GCE metadataのproject・zone・VM名、Macで得た作成応答ID、boot・netns・ifindex・VirtIO・MTU1460・内部IPを照合する。既存XDP／TC、未知のqdisc、無関係なfeature変化を拒否する。
- LRO／GRO_HW以外のSET引数を生成しない。全対象が変更可能か検査してから、一回限りのattemptを保存してSETする。失敗・結果不明で再送しない。復元は元のwantedだけを使い、activeも全体一致を確認する。未知の変更を上書きしない。
- 実NICは空BANの通常停止／SIGKILL二caseに限定。Generic一回ずつ、FD所有・実mode・PID／commandを確認し、G1完全合格前にはG2へ進まない。25分／32分／40分の開始境界と180秒を固定する。
- 非root起動gateは、九assetの読取り／hash／実行権と書込み不能、NNP・capability0、実--help／serve --helpを確認する。**GCPのservice-checkは追加で同じRust回帰実行ファイルの全51合格・一件ignoreを必須**とした。15秒のPrivateNetwork unitにAF_NETLINKの読取りを許可するが、CAP_NET_ADMIN等は追加せず、製品のsandboxは変更しない。

旧driverはSIGKILL後の自動restartを前提としていた一方、新案はRestart=noだった。この不整合を解消し、新copyはlink解放確認後に明示startする。TTL3秒・30秒・180秒、既存の待機時間・容量・stale操作の判定は維持した。旧32確認へ「明示restart前のlink解放」を一つ加えた33確認を予定する。新driverの実XDP33確認はまだ実行していない。

## 4. 検証と、残した失敗

| 検証 | 結果 |
| --- | --- |
| x86_64 build／ELF／非root実--help・serve --help | 合格。実行はQEMUで、ネイティブGCPの証明ではない |
| 新helperのoffline回帰 | **Mac27合格、native ARM Linux27合格**。mockによるSET対象・復元・不明結果の再送拒否・case順序・metadata／owner拒否・NETLINK返信検証・全asset固定・archive改竄等 |
| 旧package／access回帰 | **24合格**。旧source・証拠も保全 |
| 最終packageの非root DAC | UID／GID65534、NNP、全capability0、九fileの読取り・hash・書込み不能・製品の実--help二種を確認。QEMU／Debian12での検証 |
| 新feature reader、native ARM Linux | loの64 feature取得、LRO／GRO_HWのactive／wanted offを確認。READ-only GETLINK／GETQDISC／GETTFILTERのsmokeも合格 |
| 製品Rust library、x86_64 QEMU | **50合格・1失敗・1 ignore**。全回帰合格とはしない。失敗testはread_only_linux_get_smoke_without_attach_or_feature_setで、GETQDISC／GETTFILTER送信時EOPNOTSUPP |
| 独立C probe | 同じGETLINKはQEMU／native ARMとも成功。GETQDISC／GETTFILTERはQEMUでerrno95、native ARMで送信成功。製品だけのfailureでないことを切り分けた |
| native service gateの負例 | 同梱した同じRustテストをQEMUで実行するとgateが実際に拒否。50合格・1失敗を成功扱いにせず、BPF／SETへ進まない |

QEMUで対応しないGETを、製品guardの削除・skip・時間緩和で通していない。失敗した[元log](../../artifacts/generic-package-_y8f579p/x86-build.log)、独立probe、[gate拒否の実結果](../../artifacts/generic-package-_y8f579p/native-gate-qemu-negative.json)を保持した。C probeの最初のcompileはread-only tmpで失敗したため、新fixtureのTMPDIRだけを設定して再現し、最初の失敗logも残した。Macのmock testのplatform設定漏れも修正し、失敗記録を保持した。

**未検証:** GCPでのnative Rust51、systemd access gate、隔離33確認、ens4への実Generic装着、通常停止／SIGKILLの実FD解放・offload不変・復元、新規SSH維持、公開六条件。前段の同sourceのARM51合格や旧binaryのGCP隔離32合格を、これらへ転記しない。

## 5. 実行前の確定値と判断

project veil-x-tests、us-central1-b、e2-small一台、固定image ubuntu-2404-noble-amd64-v20261024／ID4575951068918267275。VM／diskはvlb-gcp-generic-20261027-target、専用VPC／subnet、VIRTIO_NET／MTU1460、内部10.77.28.10、ephemeral外部IPv4。管理元111.217.200.148/32→TCP22だけ。外向きIPは今回も有線en0で同値と確認した。GCP image・空き・既存物は実行直前に再照合する。

最大60分・US$0.20・税前計画約US$0.06、T0＋45分のVM／boot disk DELETEと所有照合済みnetwork撤去を維持。T0未開始、VM作成・NIC設定変更なし。scopeの生成時未実行という記録は、本人の費用意向を否定するものでも、実行許可switchでもない。

**準備成果物は固定できたが、実装全体の回帰が全環境で合格したとは判定しない。** 次の一台枠で最初にnative service-checkの同じRust51件を確認する価値がある。そこで不合格ならoffload SET／実XDPへ進まず、証拠を退避して今回VMを撤去する。合格後だけ、隔離33→B0／B1→実NIC二case→復元・撤去へ進む。公開18443・DNS・証明書・WAN BANは別判定。

VMへのNix導入は今回行わない。build再現性の選択肢としては残るが、このQEMUとnative kernel経路の違いを解消した証拠にはならない。今回の短い適合枠は、固定した成果物の実行に集中する。

文書・参照・hash・container終了の最終確認は[verification.json](../../artifacts/generic-package-_y8f579p/verification.json)へ記録する。今回の入力・cache入力（入力の内数）・出力使用量は未取得。commit／push／PR／公開・既存設定変更は行っていない。

[試験履歴](../local-ban/TEST_HISTORY.md)・[一台の計画](../local-ban/GCP_GENERIC_SINGLE_VM_PLAN_20261027.md)・[helper操作手順](../../scripts/generic-trial/README.md)へ戻る。
