# GCP装着方式の採否：Genericは条件付き候補、offload保全なしの一行変更は不採用

後続: 本人が事前offの専用環境案を採用し、[明示Generic・保全guardの実装と非課金回帰](2026-10-27-GENERIC-OFFLOAD-GUARD-IMPLEMENTATION.md)まで完了。以下は実装前の判断として保全する。新loaderの実NIC／GCP受入れは未検証のまま。

2026-10-27 JST。Sol／High、分担なし。本人の指示どおり、一次資料・既存code・[保存したGCP再試験](2026-10-27-GCP-ISOLATED-XDP-PASS-NIC-EINVAL.md)だけで評価した。**実装・build・機能再試験・GCP API／SSH／container実行・設定変更・追加課金対象の作成なし。**

最終ゴールは小規模公開サイトへの安全な手動一時BANの限定導入。現在のマイルストーンは実NICの適合方式の選定。今回の完了条件は、EINVALの分岐・Genericの副作用・FD所有と期限の維持を照合し、最小案の採否と未確定条件を明示すること。

**判断：明示Generic＋装着前のoffload保全確認を、条件付きの設計候補とする。保存されたGCPのrx-gro-hw:onを維持したまま、フラグだけSKB_MODEへ変更する案は不採用。次の有料再試験には進まない。** Genericの装着はkernel内でLRO／hardware GROを無効化する処理を呼び、link解放だけではその設定を復元しないためである。EINVALの実機の根本原因は未確定のまま。

## 1. 根拠の版と確度

| 根拠 | 今回確認した範囲 |
| --- | --- |
| 現行製品 | [kernel.rs](../../veil-node/src/local_ban/kernel.rs)はXdpFlags::empty()。Linux>=5.9、FD所有link、同じmap／object、有限BOOTTIME期限を要求 |
| Aya | Cargo.lockと手元の0.13.1。crateのVCS情報は2791badd947e3abb459e5339a23a66d0a56c42d0。公開aya-v0.13.1 sourceとも照合 |
| Linux | upstream v7.0のnet/core/dev.c、drivers/net/virtio_net.c、UAPI if_link.h。取得sourceをhash保全し、VirtIO sourceはGitHubとkernel公式mirrorで同一bytesと確認 |
| GCP実測 | Linux7.0.0-1014-gcp、virtio_net、MTU1460、combined最大2／現在2、装着失敗後のrx-gro-hw:on、LRO:off[fixed]、bpf_link_createのEINVAL（22） |
| 限界 | 実機の完全なUbuntu package revision／version_signature・交渉済みVirtIO feature bit・wanted feature集合・driver extack本文は保存していない。upstreamと実バイナリの完全同一性を証明していない |

Ubuntuの同ABI候補も探したが、該当source packageを取得できず、upstreamを当該Ubuntu patchそのものとは表現しない。取得時の429は公式kernel mirrorで補い、TLS検証失敗を無効化して読み進めていない。[sourceの版・取得限界](../../artifacts/gcp-attach-design-0nwnandh/source-scope.json)・[取得URLとhash](../../artifacts/gcp-attach-design-0nwnandh/sources-mirror.json)・[VirtIOのmirror照合](../../artifacts/gcp-attach-design-0nwnandh/source-pinning.json)を保存した。

## 2. 現行の装着経路とEINVAL

Aya 0.13.1はLinux>=5.9でflagsをBPF_LINK_CREATEへ渡し、FDを保持する。失敗後のnetlinkやGenericへの自動再送はない。[Aya同版の装着経路](https://github.com/aya-rs/aya/blob/aya-v0.13.1/aya/src/programs/xdp.rs#L130-L176)。

Linux v7.0では、mode指定なしはndo_bpfがあるdeviceならdriver経路、なければGeneric経路となる。VirtIOはndo_bpfを実装するため、参照source上では現行のflags0はdriver経路へ向かう。SKB_MODEを明示するとgeneric_xdp_installが選ばれる。[mode選択](https://github.com/torvalds/linux/blob/v7.0/net/core/dev.c#L10218-L10238)・[VirtIOのnetdev_ops](https://github.com/torvalds/linux/blob/v7.0/drivers/net/virtio_net.c#L6190-L6205)。これは失敗した実NICでnativeが稼働したという実測ではない。

| 候補・説明 | 一次sourceと保存データから言えること |
| --- | --- |
| flags0が常に不正 | 否定。0は有効なmode選択。FD linkはmode bitを受理する。一方UPDATE_IF_NOEXIST／REPLACE等の追加はこの経路で不適切 |
| GenericはFD所有linkで使えない | 否定。SKB_MODEと同じlink経路を使える。既存HPのGeneric／link FD実測もあるが、GCPでの成功は未検証 |
| VirtIOのheader配置 | mergeable RXでany_header_sgがない場合にEINVAL。実機の必要bitが未取得なので候補の域を出ない |
| MTU | driverはbuffer条件を超える場合にEINVAL。MTU1460だけから実機分岐を確定しない。fragsや実buffer条件の情報が不足 |
| queue不足 | v7.0の該当処理は追加queueを確保できない場合、locked TX方式へ進む。最大2／現在2だけで今回のEINVALと断定できない |
| guest offload制御 | CTRL_GUEST_OFFLOADSがなく所定のguest featureがある場合の戻りはEOPNOTSUPP。観測したEINVALとの違いを無視しない |
| hostのeBPF一律禁止 | 隔離実XDPの32確認合格と整合しない。実NICの条件と分ける |

VirtIOの条件とerrnoは[virtnet_xdp_set](https://github.com/torvalds/linux/blob/v7.0/drivers/net/virtio_net.c#L5967-L6020)、FD linkの既存program保護とmode制約は[dev_xdp_attach](https://github.com/torvalds/linux/blob/v7.0/net/core/dev.c#L10403-L10531)を参照。候補を絞った結果であり、どのreturnが実機で実行されたかの再現ではない。

driverの説明はextackへ渡されるが、このBPF syscallは通常のnetlink応答として説明本文を返すものではない。Ayaのこの経路はOS errnoを記録する。v7.0では失敗時の説明をtracepointへ渡す処理もあるが、当該Ubuntuでの存在・有効化・取得は未確認。[bpf_xdp_link_attach](https://github.com/torvalds/linux/blob/v7.0/net/core/dev.c#L10675-L10718)。Rust側の時刻logを増やすだけでは、欠落したdriverの説明は復元できない。今回はtraceを有効化していない。

## 3. Genericの副作用はFD所有とは別

v7.0のgeneric_xdp_installは最初の装着時にnetif_disable_lroとdev_disable_gro_hwを呼ぶ。これらはwanted_featuresを変えてfeatureを再計算し、LROについてはlower deviceへも処理が及ぶ。解除側に旧offload設定を保存・復元する処理はない。[Generic装着](https://github.com/torvalds/linux/blob/v7.0/net/core/dev.c#L6329-L6355)・[offload無効化](https://github.com/torvalds/linux/blob/v7.0/net/core/dev.c#L1823-L1857)・[XDP所有FDを閉じる際の処理](https://github.com/torvalds/linux/blob/v7.0/net/core/dev.c#L10535-L10578)。

したがって、**ethtoolのSETを自分で呼ばなくても、装着がNIC設定の変更を伴い得る。SIGKILLでlinkが外れることと、offloadが元へ戻ることは同一ではない。** 今回保存されたrx-gro-hw:on状態に対し、「一行変更だけで原状を保全できる」という保証はできない。直前のGCP実NIC試験では明示Genericを要求しておらず、装着も失敗した。そこでこのGeneric処理が実行されたという実測はない。

Genericはskbを使う後段の処理で、driver内の早いdropと同等のCPU／memory負荷軽減を約束できない。offload無効化も既存通信へ影響し得る。追加stateやWALを導入する必要はないが、線速DDoS防御や性能改善の証拠にはしない。

**既存のdefault経路でも、driver次第で暗黙にGenericへ入る。** この注意は新しいmode指定だけの問題ではない。HPのGeneric LAN合格は保全するが、旧記録だけでoffloadのactive／wanted前後一致まで確認済みとはしない。将来の保全guardは全ての装着経路に必要となる。

## 4. 最小案の採否

| 案 | 判断 |
| --- | --- |
| 同じflags0でGCPを再試行 | 不採用。実NIC失敗を覆す新しい条件がない |
| 一行だけSKB_MODEへ変え、保存したon状態を保全したことにする | 不採用。kernel内のoffload変更を見落とす |
| 明示Generic＋起動前のoffload保全guard | **条件付きの設計候補**。既に必要offloadが無効の専用環境に限る。実装・成功とも未確認 |
| アプリが自動でoffloadを変更し、終了時に戻す | 最小案では不採用。SIGKILL・他管理者の変更・root権限保持・原状回復の別設計が必要になる |
| netlinkへのfallback／既存programの置換／安全条件緩和 | 不採用。FD所有・既存program保全・単一試行を維持する |
| NICをgVNICへ変更、別host・機種を追加 | 今回保留。条件を広げた別の案で、最小差分の成功を確認したことにはならない |

### 条件付き候補の具体形（未実装）

1. 起動時に固定するmodeをdefault／genericの二値とし、defaultのflags値は現行0、genericだけSKB_MODEとする。自動fallback・REPLACE・UPDATE_IF_NOEXIST追加・pin・別のlink方式は導入しない。
2. 装着前に同じnetwork namespace／ifindexについて既存XDP・TC・単純なdevice topology・feature情報を読取り確認する。LRO／GRO_HWの**activeとwantedがともにoff**の場合だけ進む。欠落・不明・onなら、offloadを変更せず装着前に拒否する。defaultが暗黙にGenericとなる場合も守るため、このguardはdefaultも含めた全attachに適用する設計とする。これにより従来受理した一部構成を拒否し得る互換性変更を明記する。これはGenericの既知副作用へのguardで、nativeの任意driverの全設定不変を保証するものではない。候補の受入れは明示Generic構成に限定する。
3. activeだけでなくwantedも確認する。kernelの[FEATURES_GET](https://docs.kernel.org/networking/ethtool-netlink.html#features-get)は両方を返す。既存のethtool表示だけを完全なwanted集合の証明とせず、boundedなGETの取得・解析が必要。SET・watchdog・設定復元daemonは追加しない。
4. 単純な専用NICと、他の管理者が同時にfeatureを変更しない運用を前提にする。read→attachを単一の原子的な「設定不変」操作にはできないため、同時変更される共用環境の原状保証には使わない。
5. FD所有・既存map ABI・BOOTTIME絶対期限・有限TTL・256件／満杯拒否・分割掃除・instance／revision・restartでBAN0・全thread権限破棄を維持する。装着後は実mode・link FD／program ID・feature差分を確認してからREADYとする。
6. 差分や失敗があれば所有linkを解放して停止し、機能試験へ進まない。確認外のNIC設定を自動で復元して成功扱いにしない。将来の試験では通常停止とSIGKILL後のlink解放に加え、featureのactive／wanted不変を独立確認する。

この案はdriver固有のnative制約を避ける経路を明示するが、GCPで必ず装着できる保証ではない。**保存されたon状態のままではguardが拒否する。** 専用の新環境を事前にoffへ構成するなら、対象・全通信への影響・復旧方法を示す別承認が必要で、以前の「offloadを変えない」承認を流用しない。厳密に変更なしを維持して該当環境を用意できないなら、GCP実NICの試験は保留する。

## 5. 次に必要な情報と終了条件

実機の根因確定に不足するのは、kernel package revision／version_signature、交渉済みVirtIO feature、特にMRG_RXBUF／ANY_LAYOUT／VERSION_1／CTRL_GUEST_OFFLOADS、featureのactive／wanted等である。今回はVMを作り直して集めない。将来別承認の環境を使う場合の事前読取り項目として記録する。

次の判断は、**事前条件を満たす専用環境でGeneric候補を進めるか、NIC設定の変更なしを厳守してこのGCP経路を保留するか**。現条件での一行修正・課金再試験はNO-GO。候補を採用する場合だけ、別指示で最小実装と非課金検証、その後は別承認の実NIC受入れ、合格した場合だけ公開六条件へ進む。

今回の調査・採否判断は完了。追加機能実装・再試験は行わない。[分析要約](../../artifacts/gcp-attach-design-0nwnandh/analysis-summary.json)と、既存source／証拠の[保全・文書検査](../../artifacts/gcp-attach-design-0nwnandh/document-verification.json)を保持する。過去の失敗・成功の原記録は変更しない。

モデル／推論: gpt-6.1-sol／high、分担なし。入力・cache入力（入力の内数）・出力使用量は未取得。今回実施した検証は文書・参照・保全確認だけで、機能・kernel・NIC試験ではない。

[試験履歴](../local-ban/TEST_HISTORY.md)・[対応表](../local-ban/SUPPORTED_ENVIRONMENTS.md)へ戻る。
