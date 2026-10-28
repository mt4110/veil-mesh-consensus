# hp-ubuntu：owner限定隔離試験の結果と保全確認

2026-10-27 JST。本人が承認した固定範囲で**起動要求一回、owner限定試験が成功し、終了後の独立照合と証拠退避まで完了した。** BAN・chain・rule・set・hookは作成していない。nftablesでのBAN適用はBLOCKEDを維持する。

後続の[BOOTTIME採否判断](2026-10-27-NFT-BOOTTIME-ADOPTION-DECISION.md)では、現行期限を維持する標準nftablesへのBAN移植を不採用とした。以下のowner成功・回収・保全という観測結果は変更せず保持する。

最終ゴールは公開サイトで安全に使える手動一時BAN。現在のマイルストーンはnftables候補の停止・回収機能の評価。今回の完了条件は、切り離したnamespaceでsocket close／SIGKILLによる所有tableだけの回収、対照tableの存続、最後のtable0、unit停止・非enabled、既存状態の照合と記録。

## 実施した固定範囲

| 項目 | 実施内容 |
| --- | --- |
| host | hp-ubuntu、Ubuntu 26.04.1、x86_64、kernel 7.0.0-38-generic、systemd 259.9 |
| 起動 | 2026-10-27 05:37:26 JSTに一回。再送・再試験なし |
| 新設先 | `/run/veil-nft-owner-probe-20261027a/`、root0700。既存不在を確認して作成 |
| 配置 | 検証済みscriptと、配置時にrootが採取したhost-reference.jsonをroot0400で新規作成。evidenceはroot0700 |
| unit | `veil-nft-owner-probe-20261027a.service`、transient。PrivateNetwork、loのみ、CAP_NET_ADMINのみ、NoNewPrivileges、RestrictNamespaces等を維持 |
| 上限 | RuntimeMaxSec60秒、TimeoutStopSec5秒、Restart=no、TasksMax8、MemoryMax64M。変更なし |
| table | 専用namespaceのinet familyにownerだけの空table最大2個。persistなし、use=0 |

配置したscriptのSHA-256は`114d3e304d7e88758edf256a62c20e611ac3c74eda9b202838ab5bd4578b2455`。前回offline検証したsource・今回の配置前source・hostへ配置したsource・退避したsourceの一致を確認した。実行時に修正したコードはない。

## 実kernelで得た結果

| 確認 | 実観測 |
| --- | --- |
| 参照ファイル・隔離 | root0400の参照を読み取り、hostと異なるnet namespace、同じpid／user namespaceを確認 |
| 実権限 | CapEff／CapPrm／CapBndはすべて4096（CAP_NET_ADMINのみ）、NoNewPrivs=1、専用cgroup、interfaceはloのみ |
| socket close | 子processが所有socketを閉じた後も生存している間に、別socketからowned table不存在・guard table存続を確認。その後の子正常終了はstatus0 |
| SIGKILL | 新しい子だけをSIGKILLし、wait status9。別socketからowned table不存在・guard table存続を確認 |
| 最終 | guard socketを閉じた後、観測socketの完全なdumpでtable0個 |
| unit | 正常終了後にmanagerがtransient unitを解放。LoadState=not-found、ActiveState=inactive。is-enabledはnot-found／exit4で、enabled状態ではない |
| 外側の回収確認 | unit cgroup不存在、lsnsの観測範囲に今回のprivate net namespaceなし。新規SSHとsudo読取りも成功 |

Netlink応答はowner port ID・flags・family・useと完全なdump終端を照合した。所有socket以外に保持した対照tableが残るため、namespace全体の消滅だけを見て成功にした試験ではない。

unitは既に解放済みだったため、追加のstop／disable操作は不要だった。host全体のflush、未知table削除、他unit停止、host再起動はしていない。namespace参照の確認は今回のprocess終了・cgroup不存在・lsns観測の組合せであり、全ての第三者FDを走査した主張ではない。

controllerの起動からsystemd-runの完了待ちまでは約0.157秒だった。**一回の小さな空table試験の値であり、製品性能・障害時の回収時間・BAN応答時間の保証ではない。** 60秒／停止猶予5秒という上限は維持した。

## 終了後の独立照合

05:38:08 JSTに別の新規SSHで照合した。

- hostのlink詳細、nft ruleset、稼働service名が実行前と一致。既存稼働serviceのMainPID・InvocationIDを含む取得結果も完全一致。
- boot IDが一致し、host再起動なし。物理NICの変更なし。
- address／routeの生JSONは一致しないが、全差分はvalid_life_time、preferred_life_time、expiresの減少だけ。取得したIP・routeのその他の項目に差なし。
- unitは不存在・inactive、非enabled、専用cgroupも不存在。今回のnamespaceはlsnsに残っていない。
- 参照・script・実行結果・前後状態・一回起動marker・controller記録の7ファイルをMacのprivate artifactsへ退避し、それぞれhost側とSHA-256を照合。
- 既存製品の主要七ファイルは今回開始時hashと一致。BFT・WAL・XDP object・workspace／lockfileを変更していない。

これは取得した設定・service同一性の照合であり、host上の全既存データのhash監査ではない。通常のSSH／sudo／systemdの監査記録等は生じる。Pi・router・cloudへ接続していない。account・常設unit・enable・package導入・module設定・ネットワーク設定・EFIは変更していない。

## 証拠

privateな原出力を含む[今回のdirectory](../../artifacts/nft-owner-live-f_nj_lxp/)へ保存した。IP・service・namespace識別情報を含むため、公開用資料へ自動転載しない。

- [集計と採否境界](../../artifacts/nft-owner-live-f_nj_lxp/result.json)
- [実probe結果](../../artifacts/nft-owner-live-f_nj_lxp/remote/evidence/result.json)
- [起動・終了・journal・非enabled](../../artifacts/nft-owner-live-f_nj_lxp/remote/evidence/run-controller.json)
- [一回だけの起動marker](../../artifacts/nft-owner-live-f_nj_lxp/remote/evidence/one-attempt.json)
- [事前のhost状態](../../artifacts/nft-owner-live-f_nj_lxp/remote/evidence/host-before.json)・[独立した事後状態](../../artifacts/nft-owner-live-f_nj_lxp/remote/evidence/host-after.json)
- [照合結果](../../artifacts/nft-owner-live-f_nj_lxp/audit-summary.json)・[生JSONの全差分](../../artifacts/nft-owner-live-f_nj_lxp/host-differences.json)
- [退避7ファイルのhashとmode](../../artifacts/nft-owner-live-f_nj_lxp/retrieved-manifest.json)

hostの新設directoryと証拠は削除せず保全した。`/run`はhost再起動で失われるため、退避済みのMac側を併せて保管する。このfixtureを再利用して起動しない。一回だけのmarkerを消して再試行することもしない。

今回、実行用sourceの変更はなく、準備済みhashをそのまま使用した。文書全体は[98ファイルのlint](../../artifacts/nft-owner-live-f_nj_lxp/markdown-lint.log)で指摘0。ローカルリンクの参照先確認は[記録](../../artifacts/nft-owner-live-f_nj_lxp/local-link-check.json)へ保存した。文書検査をruntimeの成功条件の代替にはしない。

## 達成した範囲と残る判断

**owner限定の隔離ゲートは達成。** このhost／kernelで、root0400の事前参照と最小capabilityのfixtureが動作し、socket close／SIGKILL時の空table回収を実確認できた。この目的の追加試験を自動継続しない。

**nftables版BANの採用・公開pilotは未達。** 現行BOOTTIME絶対期限との同等性、実SO_PEERCREDと権限分離broker、manager停止連動、rule／setと実packet、容量・期限・解除・restartは今回の対象外。同じ期限契約を保つ方法が確認できなければBAN移植は不採用とする。今回の成功を理由に相対TTLへ変更したり、追加のhost／公開試験へ進んだりしない。

[承認された試験案](../local-ban/NFT_OWNER_PROBE_PLAN.md)・[前段の最小修正](2026-10-27-NFT-OWNER-HOST-REFERENCE.md)・[全体の試験履歴](../local-ban/TEST_HISTORY.md)。

モデル／推論: gpt-6.1-sol／high。分担なし。今回の入力・cache入力（入力の内数）・出力使用量は未取得。追加のcloud課金試験なし。
