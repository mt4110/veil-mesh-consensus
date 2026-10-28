# 標準nftablesへの移植：BOOTTIME絶対期限の採否判断

2026-10-27 JST。本人の指定は、一次資料と既存コードから採否を判断し、実装・設定変更・追加試験を行わないこと。今回、SSH・sudo・probe・cargo test・cloud操作は行っていない。変更は判断文書と索引のみ。

## 結論

**不採用。現行のBOOTTIME絶対期限を同等に維持する移植先として、標準nftablesのnative rule／set timeout方式は採用しない。期限契約は変更しない。**

owner限定の隔離試験は成功したが、所有socketの終了時の回収と、socketが開いたまま管理processが止まった場合の期限保証は別である。後者を同じ契約で満たせないため、broker実装・BAN rule／set・追加の実機試験へ進まない。未実装箇所を、そのまま今後の必須タスクへ積み残さない。

最終ゴールは、普通のブラウザが使う小規模公開サイトで安全に運用できる手動一時BAN。現在のマイルストーンはnftables候補の採否判断。今回の完了条件は、現行契約・標準API・代替案の限界を根拠付きで確定し、残タスクを整理すること。**この採否判断は完了、公開pilotは未達。**

## 1. 判定の範囲と根拠の確度

比較したのは、既存ローカル一時BANのsource、Linux upstream v7.0のnf_tables／meta／set lookup／UAPI、Netfilter公式manとLinuxのclock仕様である。過去のv6.8だけに依存せず、v7.0でも期限の構成方法を確認した。

前回のhp-ubuntuは7.0.0-38-genericだったが、**今回は接続していない。Ubuntu downstreamの全patchや実行中binaryを再監査した主張ではない。** 標準公開APIで現行契約を保証できるかという採用判断であり、全ての改造kernel・将来API・BPF併用方式が数学的に不可能と証明するものではない。必要な同等性が成立しない候補を採用しない、という判断である。

[一次sourceの取得URL・hash・取得時刻](../../artifacts/nft-deadline-decision-dnxlpeav/primary-sources.json)を保存した。kernel sourceは固定tag v7.0の7ファイル。実行・build・kernelへの入力はしていない。

## 2. 現行コードが守る期限

| 段階 | 実装と意味 |
| --- | --- |
| 時刻採取 | [runtime.rs](../../veil-node/src/local_ban/runtime.rs#L327)でmutationを一件受け付け、状態lock取得後にboot_nowを読む |
| clock | [kernel.rs](../../veil-node/src/local_ban/kernel.rs#L138)で非zeroのboottime namespace offsetを拒否し、CLOCK_BOOTTIMEをns単位で読む |
| 期限確定 | [mod.rs](../../veil-node/src/local_ban/mod.rs#L205)で読み取ったnow＋TTLから絶対期限を決め、同じ値をMapへ渡す。backendへの待機後にTTLを足し直さない |
| 有限期限 | [kernel.rs](../../veil-node/src/local_ban/kernel.rs#L93)のlocal backendはexpires_at_boot_ns > 0を要求する |
| packet判定 | [C XDP](../../veil-ebpf/src/veil_xdp_kern.c#L133)で、そのpacketの判定時にbpf_ktime_get_boot_nsを読み、期限より前だけ当該BANのDROP条件になる |
| 掃除 | [sweep](../../veil-node/src/local_ban/mod.rs#L252)は満了entryの物理回収。packet側の期限判定を掃除完了まで待たせない |

管理処理が止まっても、packet判定時に読んだBOOTTIMEが期限以上なら、**そのBANを理由にDROPしない**。他の防御による遮断や、packet処理全体の終了時刻まで保証する契約ではない。

bpf_ktime_get_boot_nsはsuspend時間を含むBOOTTIME相当のns時刻である。[helper仕様](https://man7.org/linux/man-pages/man7/bpf-helpers.7.html)・[kernelのclock仕様](https://docs.kernel.org/core-api/timekeeping.html)。共有Cには旧用途のzero期限経路も残るが、今回比較するlocal backendは正の有限期限しか挿入しない。その旧経路をlocal契約の無期限許容と扱わない。

## 3. 標準nftablesのtimeout／expires

公式manの`expires`は満了までの残期間である。絶対BOOTTIMEを入力するfieldではない。[Netfilter公式man：element options](https://www.netfilter.org/projects/nftables/manpage.html)。

Linux v7.0でも、NFTA_SET_ELEM_EXPIRATIONはmillisecond値からjiffiesへ変換され、新しいelementは**kernel初期化時のget_jiffies_64に、その相対期間を足して**期限を作る。既存elementの期限更新も、更新時の現在jiffiesへ相対期間を加える。[入力の変換](https://github.com/torvalds/linux/blob/v7.0/net/netfilter/nf_tables_api.c#L7248)・[element初期化](https://github.com/torvalds/linux/blob/v7.0/net/netfilter/nf_tables_api.c#L6688)・[期限更新](https://github.com/torvalds/linux/blob/v7.0/net/netfilter/nf_tables_api.c#L6984)。

kernel内部に期限の数値が保存されることと、userspaceで決めたBOOTTIME絶対期限を渡せることは別である。確認したUAPIと実装に、当該絶対値とclock IDをそのまま指定するmodeはない。expiration=0も満了済みの絶対指定にはならず、初期化ではtimeout値を使う分岐になる。

### 遅延だけで成立する反例

単位をnsに揃え、以下を置く。

- `D`: Rustが決めたBOOTTIME絶対期限。
- `s`: 残期間を計算した時点のBOOTTIME。
- `r = D - s`: kernelへ渡す正の残期間。
- `δ`: その計算後、kernelが期限を設定するまでの正の待機時間。

比較をnftables側に有利にして、clock差・丸め・suspendを全て無視しても、kernelが設定する期限は`(s + δ) + r = D + δ`になる。したがって、送信直前に残期間を計算しても、元の期限を越える場合がある。

例えばD=30秒、s=10秒、r=20秒、kernel初期化が11秒なら期限は31秒になる。**これはsourceから構成した反例で、1秒の遅延を今回測定したものではない。** 既存のoffline反例も再実行していない。

単一mutation・短いqueue・高速なhost・ACK待ち上限は、残期間計算後の待機を常に0にする保証ではない。ACK後の再読取りや削除にも待機・freezeがあり、packet側の絶対期限判定を置き換えられない。この理由だけで不採用にでき、実機suspendでの反例再現は不要である。

### GCと期限判定は区別する

nftablesがGC完了まで必ずDROPし続けるという説明はしない。v7.0のtimeout対応rhash lookupは、検索時に期限切れを判定して候補から除く。[rhash lookup](https://github.com/torvalds/linux/blob/v7.0/net/netfilter/nft_set_hash.c)・[期限比較](https://github.com/torvalds/linux/blob/v7.0/include/net/netfilter/nf_tables.h#L840)。公式manもgc-intervalはメモリ回収に影響し、element timeoutとは別としている。

問題は掃除の遅さと断定できることではなく、**比較に使う期限そのものを現行Dと同じ契約で設定できないこと**である。ms／jiffiesへの丸めもns絶対比較との差になるが、今回の結論は丸め幅やsuspend中のjiffies進行の実測に依存しない。

## 4. 回避案の採否

| 案 | 判定と理由 |
| --- | --- |
| timeoutをそのまま渡す | 不採用。kernel側の開始時点になり、元のDと異なる |
| expiresへ送信直前の残期間を渡す | 不採用。その後の待機分を吸収できない。絶対値APIではない |
| 安全マージンを引いて早く失効 | 不採用。必要な最大待機時間の保証がなく、期間短縮も同等契約ではない |
| ACK後に読取り・補正を繰り返す | 不採用。補正自体に待機があり、process停止中の期限保証にならない。無限retryも認めない |
| meta timeで絶対日時と比較 | 不採用。CLOCK_REALTIME系の壁時計で、BOOTTIMEと異なる。clock変更で期限が早まる／延びる |
| 200ms掃除・timerfd・systemd timerで削除 | 不採用。削除を行うprocessやkernel処理の待機で期限を越え得る。packet側の独立した絶対判定の代わりにならない |
| ownerだけで失効保証 | 不採用。socket解放の回収には使えるが、processがfreezeしてFDを保持する場合のDを表現しない |
| kernel適用時をTTLの起点に変更 | 別契約。一般的な相対timeout用途の候補にはなるが、本人が選んだ現行期限維持と一致しないため今回採用しない |
| nftables＋独自BPF／kernel拡張 | 今回の標準nftables移植には含めない。技術的可能性まで否定しないが、新しい実装・権限・保守・検証を必要とする別案件。自動追加しない |

meta timeについては、v7.0のNFT_META_TIME_NSがktime_get_real_nsを読むことを確認した。[metaの実装](https://github.com/torvalds/linux/blob/v7.0/net/netfilter/nft_meta.c#L60)。REALTIMEはNTPや時刻設定等で不連続に変わり得るため、固定offsetでBOOTTIMEへ変換した値を保持するだけでは同等にならない。[clock仕様](https://docs.kernel.org/core-api/timekeeping.html)。

BPF併用の例としてxt_bpfにはsocket-filter typeのBPFを受け入れる実装があるが、現在のXDP objectをそのまま使う方式ではない。[xt_bpfのtype検査](https://github.com/torvalds/linux/blob/v7.0/net/netfilter/xt_bpf.c#L40)。この存在を、標準nftablesだけでBOOTTIME期限を保証できる証拠にしない。TC等の新backend研究も今回の残タスクへ足さない。

## 5. 終了するもの・保全するもの

- 標準nftablesへ現行契約のまま移植する候補は、不採用として調査を終了する。追加の期限実験・broker実装・公開試験を必須タスクに残さない。
- Rustの独立モデル、修正前の反例、owner probe、実機一回の成功と回収・保全記録は全て残す。削除・書換えによる過去結果の修正はしない。
- 現行XDPの製品契約・code・object・BFT／WAL・既存データ・設定は維持する。nftablesへ自動fallbackしない。
- 今回はhostの停止状態やcloudの残resourceを再照会していない。前回のunit停止・非enabledと保全は[当時の実測](2026-10-27-NFT-OWNER-ISOLATED-ACCEPTANCE.md)として参照する。

## 6. 公開pilotまでの残タスク

| 優先順 | 残る判断・作業 | 完了条件 |
| --- | --- | --- |
| 1 | 現行XDPの確認済み条件で保護できる実サイト・入口と、既存手段より導入する理由を決める | 対象・NIC／MTU・実source・保護tuple・管理復旧経路・基礎防御が揃う。価値または対応条件がなければ保留／終了 |
| 2 | 条件が揃った場合だけ、既存手順の公開受入れを具体案として承認する | 正常通信・対象だけ遮断・期限前解除・自然失効・restart／停止復帰・本人操作／保全の[六条件](../local-ban/DELIVERY_GATE.md)を、採用する同じ環境と成果物で確認 |
| 3 | 限定試験導入または終了の判定 | 証拠・未確認・運用負担を一枚にまとめ、限定導入候補か不採用かを決める |

公開経路・独立正常client等が未確定のため、残り時間の確約はしない。新しい機能実装や別cloud／NICの試験を、先に追加する必要はない。今回の不採用決定は、現行XDPの公開受入れ成功や製品価値の証明にはならない。

## 7. 文書・保全の確認

[照合結果](../../artifacts/nft-deadline-decision-dnxlpeav/result.json)では、製品・object・probe・独立モデル等の関連12ファイルが開始時hashと一致。変更した文書からのローカル参照214件に参照先欠損はなかった（anchorの全検証ではない）。文書全体は[99ファイルのlint](../../artifacts/nft-deadline-decision-dnxlpeav/markdown-lint.log)で指摘0。これらは文書と保全の検査であり、追加の機能・性能試験を行った記録ではない。

モデル／推論: gpt-6.1-sol／high、分担なし。今回の入力・cache入力（入力の内数）・出力使用量は未取得。実装・設定変更・追加の機能／性能試験・課金試験なし。実施する検査は文書のlint／参照先確認と既存sourceのhash照合だけ。
