# 明示Genericとactive／wanted保全guard：実装・非課金回帰まで完了

2026-10-27 JST。gpt-6.1-sol／high、分担なし。最終ゴールは小規模公開サイトへの安全な手動一時BANの限定導入。現在のマイルストーンは実NICの適合方式の確認。今回は、本人が採用した「LRO／hardware GROが既にoffの専用環境」向けにmodeと保全guardを実装し、追加課金なしの回帰検証を完了すること。

**実装と非課金回帰は完了。新loaderの実XDP／実NIC／GCPは未検証、公開pilotはHOLD。** hostのNIC設定・route・firewall・unitを変更せず、cloud API・SSH・課金試験も実施していない。自動fallback、offloadのSET／自動復元、BPF object変更は追加していない。

## 1. 実装した振る舞い

- `serve --xdp-mode default|generic`を追加。省略はdefaultでflags0、genericだけSKB_MODE。native失敗後の再送、netlink装着へのfallback、置換、pinはない。
- [attach_guard.rs](../../veil-node/src/local_ban/attach_guard.rs)で、**両modeとも**装着前にLRO（rx-lro）とhardware GRO（rx-gro-hw）のactive／wantedがともにoffか検証する。on・欠落・未知・読取り失敗ならBPF objectのload前に拒否する。
- ethtoolはGSSET_INFO／GSTRINGS／GFEATURESのGETだけ。名前からbitを解決し、internalなfeature bit番号を固定しない。256 featureまで、固定32 byte名、返信長／重複／欠落／padding／word数を検査する。requestedをwantedとして取得する。既存のAF_UNIX／AF_NETLINK制限と依存だけで実装し、権限を追加しない。
- 同じprocessのnetwork namespaceでifindexを解決し、rtnetlinkのGETLINK／GETQDISC／GETTFILTERだけを使用。受信はkernel送信元・sequence・長さ・件数・dump中断・時間を検査する。sysfsの別namespaceのNICを同じものと誤認しない。
- plain Ethernetと隔離vethに限定。master・下位device・VLAN／bond／bridge等のkind・不明な構成・既存XDPを拒否する。許容qdiscは最大16個のnoqueue／mq／fq_codel／pfifo_fast／fqだけとし、clsact／ingressや未知のqdiscを拒否する。fq_codelにもclassifierがあるため、許容qdiscの全chain／priorityのfilter有無をGETして、存在・取得失敗も拒否する。他ifindexのqdiscを変更・削除しない。
- [kernel.rs](../../veil-node/src/local_ban/kernel.rs)は検査済みifindexへBPF_LINK_CREATEを一回だけ実行し、返却linkをAyaのFdLinkへ変換して所有する。READY前に実mode・program ID・ifindex／name／MTU／単純構成、feature全体のactive／wantedとqdisc構造の一致を確認する。
- 後段確認に失敗した場合もRAIIで**所有linkだけ**を解放して終了する。NIC設定や他programを復元・置換しない。READYに要求mode・実mode・program IDを表示する。

標準[unit](../../deploy/systemd/veil-local-ban.service)と[config例](../../deploy/systemd/veil-local-ban.service.conf.example)、[固定recovery override](../../deploy/systemd/veil-local-ban.service.d/recovery.conf.example)は同じVEIL_XDP_MODEを使う。未指定configとの互換性のためunit既定はdefault。これはrepository内のtemplate編集であり、hostへの配置・daemon-reload・起動は実施していない。旧binaryには新しいCLI optionがないので、新templateだけを旧binaryへ適用しない。

## 2. 維持した契約と限界

BOOTTIME絶対期限・有限TTL・256件／満杯拒否・掃除予算・instance／revision・restartでBAN0・FD解放・起動後の権限破棄は維持した。guardは起動時の検査であり、packet経路や定期掃除へ追加処理を入れていない。BFT／WAL／旧object／保存済みbinary／試験packageには変更していない。

**互換性は意図的に狭まる。** defaultでも暗黙Genericへ入る可能性があるため、従来受理したNICでも必要featureがon／不明なら拒否する。GCPで保存したrx-gro-hw:on状態も拒否対象で、Genericを選ぶだけでは通らない。

GETと装着は、他の特権管理者による変更に対して原子的ではない。専用NIC・同時再構成なしが前提。postflightは不一致を検出するが、既に起きた他者の変更や任意native driverの副作用を自動で元に戻す保証ではない。qdisc確認も未知の全kernel hook／TCXの網羅保証ではないため、別の管理systemやBPF policyと共用する構成には広げない。

明示Genericはdriver内の早期dropと同等の性能を保証しない。線速・回線飽和対策・CDNの背後の実client識別・IPv6／HTTP3は今回の合格対象外。旧HP／GCPの実XDP合格を、このguard付きsourceの合格へ転記しない。

## 3. 一次資料との照合

前段の[設計採否](2026-10-27-GCP-XDP-ATTACH-DESIGN-DECISION.md)にあるGenericのoffload副作用・FD所有の分析を継承した。今回のgetterはLinux v7.0の[ethtool UAPI](https://github.com/torvalds/linux/blob/v7.0/include/uapi/linux/ethtool.h)と、[GET実装](https://github.com/torvalds/linux/blob/v7.0/net/ethtool/ioctl.c#L77-L112)に照合。GFEATURESのrequestedはdevのwanted_features、activeはfeaturesから生成される。feature名は同版[common.c](https://github.com/torvalds/linux/blob/v7.0/net/ethtool/common.c)にあるrx-lro／rx-gro-hwを用いた。

UAPIの取得sourceを[今回の証拠領域](../../artifacts/generic-guard-nos9s6bh/)に保持。最初のGitHub raw取得の429もerror fileとして残し、公式kernel mirrorの同tagから補った。実際のGET ABIは後述のローカルLinux 6.8で読取り確認したが、GCPのUbuntu kernel patchとの完全同一性や実NIC装着成功を証明したものではない。

差分確認時に、許容したfq_codelにもtcf_blockがあることを[同版source](https://github.com/torvalds/linux/blob/v7.0/net/sched/sch_fq_codel.c)から確認し、qdisc名だけでfilter不在とみなす見落としを修正した。照会は[tc_dump_tfilter](https://github.com/torvalds/linux/blob/v7.0/net/sched/cls_api.c)に合わせてqdisc handleごと、chain／priorityを絞らないGETとした。lo上の空dumpは実確認したが、今回filterを設置する実機陽性試験は行っていない。

## 4. 実施した検証

ローカルLinuxはcached rust:1.98 image、ARM64／Linux6.8.0-90-generic／Rust1.98.1。network=none、UID／GID65534、CapEff=0、NoNewPrivs=1、rootfs read-only、cap-drop=ALLで実行。実NIC装着やethtool SETはない。GET smokeはcontainer内のloのみに対して行い、Ethernetでないloへの起動は両modeともload前に拒否することも確認した。

| 検証 | 結果・範囲 |
| --- | --- |
| 最終sourceのLinuxローカルBAN library | **51合格、1 ignore**。新規guard16＋CLI1を含む。旧socket所有・SIGKILL後回収・一覧・preview・期限・256件・revision等の回帰を含む |
| 新guardの拒否境界 | activeのみon、wantedのみon、両feature、bit31／32境界、欠落・重複・malformed・上限、既存program各mode、postflightのfeature／identity／MTU／qdisc／mode／program不一致を検証 |
| Linuxの既存node回帰 | **185合格、7 ignore**。BFT・WAL・TLS等。GET専用allowlist・TC filter確認を追加する前の同作業sourceで実行。追加後はローカルBAN51を再確認し、他領域は変更なし |
| Linux製品build・非root実起動 | buildとserve --help成功。default／genericだけが選択肢。BPF loadや装着の成功ではない |
| macOS portable library／既存node回帰 | **10合格**／**182合格、2 ignore**。Linux guardや実XDPの代替証拠にはしない |
| 既存package／権限回帰 | **24合格**。旧archiveと固定hashを保全。新sourceを含めた新packageは作成していない |
| Clippy・format | macOSのlibrary／local-ban binaryは-D warningsで成功。変更Rust fileのrustfmt成功。Linux imageにはClippy componentがなく未実施。失敗logを保全し、追加installや成功扱いはしない |
| 文書・保全 | 全Markdown検査、local linkのpath確認、diff空白検査、旧object／binary／archive／lock／manifestのhash照合を実施。結果はverification.jsonに保存 |

ignoreは従来の明示的なprivileged／隔離fixture等で、今回有効化していない。単体検証の件数は実NIC受入れの代替ではない。途中のcompile・Linux Clippy利用不能を含む[log](../../artifacts/generic-guard-nos9s6bh/linux-regression.log)、[TC確認後のLinux回帰](../../artifacts/generic-guard-nos9s6bh/linux-tc-final.log)、[最終検証](../../artifacts/generic-guard-nos9s6bh/linux-acceptance-final.log)、[保全結果](../../artifacts/generic-guard-nos9s6bh/preservation.json)を残した。新規local containerは終了済みで削除せず保持し、IDと状態をverification.jsonに記録する。

今回生成したARM64 debug binaryのSHA-256はverification.jsonに固定した。旧x86_64 binaryのe15789d…とは別であり、GCPへ配布していない。依存追加・Cargo.lock変更・BPF ABI変更はない。

## 5. 残りと次の終了条件

1. **専用環境の具体案確認。** 新たなNIC事前設定の対象・影響・元のactive／wantedの保存・復旧方法、GCPの費用・期限・削除対象を提示するまで実行しない。過去の課金・offload変更なしの承認は流用しない。
2. **新成果物で隔離／実NIC受入れを一回。** 別承認後にのみ、x86_64 build・package権限・非root実起動、隔離回帰、必要feature事前off、明示Genericのmode／所有ID、通常停止・SIGKILL後のlink解放とactive／wanted不変、新規SSHを確認。失敗時にfallback・再作成・設定緩和せず保全・撤去する。
3. **実NIC合格後のみ公開六条件。** [DELIVERY_GATE](../local-ban/DELIVERY_GATE.md)による到達・対象遮断／解除・対象外・期限・restart・復旧を別承認の公開経路で確認。ここまで満たして限定導入可否を判定する。

今回タスクは完了、実NIC適合のマイルストーンと最終ゴールは未達。追加課金0件、GCP作成0件。確定請求額やaccount全体の状態を再照会した主張ではない。入力・cache入力（入力の内数）・出力使用量は未取得。実行model／effortはsessionのturn_contextでgpt-6.1-sol／highを確認。

[試験履歴](../local-ban/TEST_HISTORY.md)・[対応環境](../local-ban/SUPPORTED_ENVIRONMENTS.md)・[導入](../local-ban/INSTALL.md)へ戻る。
