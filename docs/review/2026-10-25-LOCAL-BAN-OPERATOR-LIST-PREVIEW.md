# 一時BANの一覧と手動操作プレビュー

2026-10-25 JST。Sol／High、分担なし。
最終ゴールは普通のブラウザが使う公開サイトで、安全に試用・停止・復旧できる無償OSS。
今回は、Ubuntu準備待ちでも進められる「BANを見渡す」「手動で操作する際にinstance／revisionを取り違えない」ための最小支援を実装した。
自動検知、自動BAN、自動再送、BAN永続化、ホストへの配備は追加しない。

## 追加した操作

実行先はLinuxの新しい`veil-local-ban`。以下は製品unitの予定pathを使う操作例であり、現在そのhostへ配置したものではない。
実行するhost／binary／directory／source IPを確認してから使う。例示IPを実環境へコピーしてBANしない。

```sh
sudo -u veil-local-ban /usr/libexec/veil-local-ban list \
  --control-dir /var/lib/veil-local-ban/primary

sudo -u veil-local-ban /usr/libexec/veil-local-ban preview-ban \
  --control-dir /var/lib/veil-local-ban/primary \
  --ip 198.51.100.20 --ttl-secs 30

sudo -u veil-local-ban /usr/libexec/veil-local-ban preview-unban \
  --control-dir /var/lib/veil-local-ban/primary \
  --ip 198.51.100.20
```

三つとも読取り専用。statusと同じ所有者限定の通常管理socketを使い、期限切れentryを削除することもない。
一覧取得やpreviewだけでrevision、変更rate制限、event、BAN期限は進めない。

### list

一回の応答に最大256件を含める。ページ間の不整合や継続tokenを作らない。
headerにはinstance、revision、snapshotのboot clock、宛先IPv4／TCP port、capacity、bypassの観測値等を含める。
各行はIP、`state`、`confirmed`、`expires_at_boot_ns`、残り秒数の切上げ値`remaining_secs_ceil`。

| 表示 | 意味 |
| --- | --- |
| active | snapshot時点で未失効、最後のMap挿入成功を確認済み |
| pending | 未失効だがMap挿入結果を確認できていない追跡slot |
| expired | snapshot時点で期限を過ぎたが、まだ回収されていないslot |
| confirmed | Map操作の確認状態。特定clientの通信を観測した意味ではない |
| bypass_observed | 一覧の応答作成時に別途観測した値。BAN snapshotと同時に固定したcontrol revisionではない |

`source=userspace-mirror`を明示する。**一覧のactiveは実通信の遮断保証ではない。** bypass、対象外protocol、fragment等の条件と実通信は別に確認する。
期限切れやpendingも容量を使うため、256件で新規BANが拒否される理由を隠さない。空欄や成功した空一覧へ置き換えない。
状態lockを直ちに取れなければ`ERR / BUSY`。snapshotを固定長配列へコピーした後にlockを離し、整形や送信中は保持しない。Map syscallによる全件照会はしない。

### preview-ban / preview-unban

対象IP、保護IP、時計の逆行を確認する。BANではさらに有限TTL、期限のoverflow、空slotまたは同一IPへの更新余地を確認する。
previewは状態を予約しない。rate制限、revision、Map結果等の適用時の成功を保証しない。同じ確認を実際のbanでも行う。

出力には次を含める。

- 遮断先IPv4／TCP port、対象IP、指定TTL、現在追跡中か、現在の期限。
- `effect=new-lease`、`replace-existing-lease`、`remove-lease-if-present`。同一IPへの再BANは新しい適用時刻からの期限更新になる。
- snapshot時点のinstance／revisionを埋めた`execute_manually=`のコマンド。

**右側のコマンドを目で確認して、同じservice UIDで別途実行する。** 例のようにsudoを使う場合は、そのコマンドにも`sudo -u veil-local-ban`を前置する。
preview自体は実行も再送もしない。出力を`eval`やshellへのpipeへ流さない。生成commandをcron／queueへ保存しない。control-dirは絶対pathを使い、previewから実行までhost・UID・directoryを変えない。

別の変更が入るとglobalなrevisionが変わり、古いcommandは既存の`REVISION_CONFLICT`で拒否される。別IPへの変更でも同じ。
daemonが再起動すればinstanceが変わり、旧commandは`INSTANCE_CONFLICT`。commandのinstance／revisionを自動取得し直して通す処理はない。
一度適用したBANを解除した後も、古いBAN commandの再送はrevision不一致で拒否する。

previewは期限付きの承認tokenではない。mutationも再起動もなければ、そのrevisionは時間だけでは失効しない。表示時刻、IPの所有・用途、操作意図が古くなった場合は実行せず改めて確認する。
TTLはpreview時点で開始せず、**明示的なban実行時から**数える。previewをBANの予約や発効済みの証拠として扱わない。
rate制限や容量競合で拒否された場合も自動再試行しない。結果不明の場合はstatus／listと実通信を確認して止める。

生成commandのpathはPOSIX shell用に引用し、引用符・空白・`$()`を命令として解釈させない。改行等の制御文字や非UTF-8のpathはcommand生成を拒否する。
daemon応答からcommandを作る際も、operation／IP／TTLの一致、instance／revisionの型、重複fieldを検査する。応答から任意のshell本文を実行しない。

## 維持する境界

| 項目 | 今回の扱い |
| --- | --- |
| 既存の直接操作 | ban／unbanは引き続きinstance・expected-revision必須。bypassは従来のcontrol_revisionを使う |
| wire | VLB1の固定40 bytes＋EOFを維持。既存op 0〜4は変更しない。5=list、6=preview-ban、7=preview-unbanを追加 |
| 読取りframe | 新opのinstance／expectedはゼロ必須。listはIP／TTLもゼロ、preview-unbanはTTLゼロ。他の形は拒否 |
| 旧daemon | 新opをINVALID_FRAMEで拒否する。clientは応答不正／非対応ならcommandを生成せず止める。自動fallbackしない |
| 応答量 | listだけ最大64KiB。既存操作とpreviewは4KiBを維持。上限超過は部分成功として表示しない |
| 時間・同時数 | 既存の2秒、通常8枠、緊急1枠、mutation1件を維持。list／previewは緊急socketでは拒否 |
| 容量・掃除 | 256 slot、満杯時新規拒否、16 slot／200ms、Skipを維持。取得時の全件削除やLRU追出しはない |
| 保存・再起動 | BANはRAM／FD所有Mapだけ。追加の状態file・履歴・token保存なし。新Engineは空で開始 |
| kernel | bytecode、Map ABI、TTL、service scope、FD所有link、起動権限破棄は変更していない |

## 検証と証拠

今回のsourceは`artifacts/ban-operator-fri9ct_9/`の変更前copy／manifestと最終source manifestで区別する。
macOSではportableなEngineの回帰を実行し、Linuxではpi-nodeの新規workspace、一時的な非特権build unit、read-onlyの既存vendor／runtimeを使ってbuild／library回帰を行う。
既存のsource workspace、build成果物、fixture、常設unit、account、firewall、物理NIC、既存namespaceは変更しない。Linux検証はUID1000・capabilityなしの模擬Backendと新しいUnix socketで行い、XDPを装着しない。

追加の確認点:

- snapshot／previewが状態や期限を変えず、容量・保護IP・TTL・overflowを正しく扱う。
- 256件すべての一覧が一回で返り、既存の4KiBを超えても欠落しない。64KiB超過や旧操作の4KiB超過は拒否。
- BUSYを空一覧へ偽装せず、緊急socketの分離と固定frame検査を維持。
- preview→競合変更→旧revision拒否、preview→新instance→旧要求拒否。
- previewだけでは適用されず、明示的な適用→解除後の古いBAN要求を拒否。
- 引用したcommandを実shellでargvへ戻してもIP／TTL／pathが変わらず、CLIのinstance／revision必須も維持。

結果:

| 確認 | 結果 |
| --- | --- |
| macOS / `cargo test --offline --locked -p veil-node --lib` | 10 passed / 0 failed |
| pi-node / native Linux binary build | 成功、UID1000・Eff/Prm/Inh/Amb capabilityゼロの隔離build |
| pi-node / `cargo test --offline --locked --jobs 2 -p veil-node --lib` | 34 passed / 0 failed / 1 ignored。ignoredは従来の明示SIGKILL受入れ用fixture |
| 新binaryのhelp | 全体・list・preview-ban・preview-unbanともexit 0 |
| 変更3fileのrustfmt検査 | 成功 |
| 最終sourceとbuild入力 | manifestの73fileすべて一致 |
| 一時build unit | `veil-nix-build-s2dyr97k.service`はnot-found／inactive／MainPID0。常設unitを配置・enableしていない |

最終binaryは`/home/maakie/vnr-s2dyr97k/target/debug/veil-local-ban`、SHA-256 `e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890`。
既存の配置先のbinaryは交換していない。新しいsource／成果物／logは保全する。
初回Linux回帰はtransport境界の追加前で33 passed、最終回帰はその確認を含む34 passed。両者のsource・logを別に残し、同じ版を複数回通した証拠としない。
検証記録は同directoryの`linux-final-results.json`、`linux-final-regression.log`、`mac-regression.log`、`source-final-manifest.json`等。macOSのportable回帰とLinux専用のsocket／CLI回帰を区別する。
実kernelでの新機能受入れ、公開site、実operatorの操作性は未検証。以前のkernel受入れは旧binaryの証拠として保全し、今回のbinaryの実機合格に読み替えない。
本変更は[最短経路の計画](2026-10-25-PILOT-SHORTEST-PATH-AND-VALUE-GATES.md)で挙げた一覧不足への対応であり、detector追加や本番導入の開始ではない。
