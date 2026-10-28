# BAN一覧・プレビュー操作の実XDP隔離受入れ

2026-10-25 JST。Sol／High、分担なし。
**新しい一覧・プレビュー付きbinaryをpi-nodeの実XDPへ接続し、一回の通し実行で31確認が成功した。試験終了後の停止・非自動起動、XDP非装着、通信復旧、既存環境の保全も確認した。**

最終ゴールは、普通のブラウザを使う公開サイトで安全に試用・復旧できる無償OSS。
今回のゴールは[一覧・手動操作支援](2026-10-25-LOCAL-BAN-OPERATOR-LIST-PREVIEW.md)を実際のkernel／HTTPSまで通し、その安全条件を確認すること。
本報告で前報の「このbinaryの実kernel受入れは未実施」を更新する。公開site・物理NIC・実運用者の操作性は依然として未確認。

## 対象と実行方式

| 対象 | 使用したもの |
| --- | --- |
| 新binary | `/home/maakie/vnr-s2dyr97k/target/debug/veil-local-ban` |
| 新binary SHA-256 | `e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890` |
| object | `/opt/veil-local-ban-pi-full/veil_xdp_kern.o` |
| object SHA-256 | `2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f` |
| host側の旧binary | `/opt/veil-local-ban-pi-full/veil-local-ban`、SHA-256 `f9cf2dc594cc0808b91247a8f0d447edcb7b9370ae8273e0f777deebe85da05a` |
| 隔離範囲 | 既存の`ns-veil-rogue`／`veth-rogue`のみ。宛先`10.99.0.66:18443/TCP`、対象外port18444 |
| 実行UID | 既存専用UID/GID979。account作成や権限設定の変更なし |
| 一時unit | `veil-operator-4db32e3d8d01.service` |

[新しい固定試験driver](../../scripts/local-ban-operator-kernel-check.py)から、既存の試験unitと同じhardening／起動・停止条件を持つ一時unitを作成した。
一時unitのmount namespace内だけで、新binaryを従来pathへread-only bindした。hostの旧binaryを交換・上書きしていない。
初回・通常restart後・SIGKILL後の各世代で`/proc/PID/exe`のhash、実network namespace、全threadのUID・capabilityを確認した。

管理CLIもdaemonと同じmount namespaceへ入り、UID979へ落として実行した。これにより、previewが示す実行pathと実際の新binaryを一致させつつ、host側の旧配置は保全した。
previewの出力commandはshell評価せず`shlex.split`でargvへ分解し、binary・操作・control directory・IP・TTL・instance・revision・scopeを照合してから、**別の明示的な試験操作として**実行した。
製品に自動BANを追加したわけではない。これは人間が通るCLI経路の自動回帰であり、人間の判断精度・完了時間の試験ではない。

## 確認した振る舞い

| 対象 | 結果 |
| --- | --- |
| 正常HTTPS | 装着前・空BAN・個別解除・期限失効・restart・最終停止後に、IP SAN付き証明書とrun固有bodyを検証して成功 |
| 実体と権限 | 全3世代で新binary hash一致、隔離namespace一致、全threadがUID979／Eff・Prm・Inh・Ambゼロ／NoNewPrivs=1 |
| previewのみ | BAN件数・revision・event countが変わらず、HTTPSも継続 |
| previewからの明示適用 | 一覧に実送信元IPがactive／confirmedで現れ、対象HTTPSが遮断。kernel drop counter増加、対象外18444は継続 |
| 古いrevisionの解除 | unbanのpreview後、別IPへ明示的にBANしてrevisionを進めた。古い解除commandをREVISION_CONFLICTで拒否し、元IPの遮断を維持 |
| previewからの明示解除 | 新しいpreviewのcommandで解除し、HTTPS復帰。一覧からも対象IPが消えた |
| 解除後の古いBAN再送 | 最初のBAN commandをREVISION_CONFLICTで拒否し、解除した通信を再遮断しない |
| 無効なpreview | 保護IPとTTL=0を拒否し、適用commandも出力しない |
| kernel期限 | TTL3秒を明示適用して遮断・未失効を確認し、daemonをSIGSTOP。停止中でも期限経過後にHTTPS復帰 |
| 分割掃除 | SIGCONT後の既存4秒観測内に空一覧／occupied0。max_examined／max_attemptedは16以下 |
| 256件一覧 | 既存の変更間隔・TTL180秒で一件ずつpreview→明示適用。実送信元を含む256件すべてがactive／confirmed、重複・欠落なし。4KiB超の応答も取得 |
| 257件目 | previewをCAPACITYで拒否しcommandを出さない。直接の明示BANもCAPACITY。256件のIP／期限／revisionを維持、元の遮断と対象外通信も維持 |
| 通常restart | 256件を保持した状態からrestartし、新instance・一覧0件・occupied0・HTTPS復帰 |
| 古いinstance | restart前のBANとunbanのcommandをどちらもINSTANCE_CONFLICTで拒否し、一覧は空のまま |
| SIGKILL | 新たにpreviewからBANして遮断後、mainをSIGKILL。既存のRestart条件で新instanceへ再起動、空一覧とHTTPS復帰 |
| 明示停止 | 所有unit停止後にXDP非装着とHTTPS復帰を確認。停止要求の返却だけでは合格にしていない |

一回の実行で31確認すべて成功、`complete=true`／`cleanup_ok=true`。
CLI呼出しは564回、うち変更命令267回には意図した拒否5回を含む。これらは品質を件数で示すためではなく、試験入力と拒否の内訳を追跡するための記録。
変更命令の再試行・instance／revisionの自動更新・途中再開はない。読取りBUSYだけの有限再照会を許すdriverだが、この実行のBUSY応答は0回だった。
TTL、256件上限、2秒の管理I/O期限、既存の起動・停止・restart条件は変更していない。

## 終了と保全の確認

- 新unitは`not-found / inactive / MainPID=0`、自動起動symlinkなし。**一時unitをdisabledと表示したという意味ではなく、常設配置・enableを行わず終了後に消滅した。**
- 既存4 unitはすべてinactive／disabled／MainPID0を維持。
- 別の読取り監査で、隔離linkのXDP非装着、namespace内PIDなし、18443／18444待受なし、UID979のprocessなしを確認。
- 新規control directoryに残るのはsocket-ownerのみ。未知のsocket／fileは削除・修復しない。
- 既存test／full／boot fixtureのmetadata・hash、主要serviceの設定hash・状態・PID、mainnet情報、public account情報、boot IDは前後一致。
- host interfaceのifindex／name／MAC／MTU／flags／XDP、アドレス構成、IPv4経路、IPv6経路の構成、IPv4／IPv6のfilter rule、UFW表示も一致。
- IPv6 routeの有効期間等の動的値は構成比較から除外した。生のbefore／afterも保持しており、アドレス・経路の変更を隠すための全体除外はしていない。
- 新旧binary／objectのhashが前後一致。前回build入力manifestの73fileと現在のsourceも一致。今回製品codeを修正していない。

新規作成は/runのdriver・runtime・証拠、一時unit、試験用証明書／keyだけ。通常のsystemd／journalのlogは発生する。
hostのaccount、常設unit、firewall、interface／route、既存namespaceのアドレス構成、Nix mount、時刻設定、既存配置物は変更していない。host rebootも行っていない。
これは対象を絞った前後比較であり、host上の全dataの論理的整合性を完全証明したものではない。

## 証拠と完了範囲

- local: `artifacts/operator-kernel-o92pdz60/`。
- [集約](../../artifacts/operator-kernel-o92pdz60/acceptance-summary.json)、[全CLIと確認結果](../../artifacts/operator-kernel-o92pdz60/result.json)、[独立した最終監査](../../artifacts/operator-kernel-o92pdz60/independent-final-audit.json)。before／after、command、journal、実行log、driverとsource hashも保全。
- remote: `/run/veil-operator-evidence-1f5kp6rc/`、runtime `/run/veil-operator-runtime-zxujutjy/`。今回の所有物も手動削除していない。/runは次のhost rebootで失われるため、必要な非秘密の証拠はlocalへ取得済み。
- 試験用秘密keyの本文は取得・表示していない。

本タスクは完了。前回のLinux library回帰34成功／明示fixture1件除外は同じsourceの別の証拠で、今回は再実行していない。
今回追加したのは、そのbinaryの実XDP・HTTPS・CLI経路の受入れ証拠である。
次の[公開pilotのgate](2026-10-25-PILOT-SHORTEST-PATH-AND-VALUE-GATES.md)には、実host／物理NIC、公開経路、管理source、独立clientと既存防御の確定が必要。
vethのIPv4/TCP成功を、全NIC・IPv6・CDN越し・fragment・自動攻撃検知・既存手段に対する優位性の証拠へ広げない。
