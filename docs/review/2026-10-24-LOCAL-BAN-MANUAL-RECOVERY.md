# ローカル一時BAN: 初期化中断・所有記録破損と手動復旧

2026-10-24。[SIGKILL後の自動復旧](2026-10-24-LOCAL-BAN-CRASH-RESTART.md)に続き、初期化途中で停止した場合と所有記録が壊れた場合の扱いを確認した。
最終ゴールは、小規模公開サイトで安全に運用・復旧できる無償OSS。今回のゴールは、所有不明のものを保全して拒否すること、変更しない診断手段と手動復旧手順を用意し、回帰と隔離Linuxで確認すること。

## 契約

**所有が証明できなければ元のdirectoryを変更せずに拒否する。手動復旧は、明示的に選んだ別の空directoryへ管理入口を切り替える。**
元のsocketやfileの削除・rename・checksumの修復・所有記録の作り直しを、復旧条件の代わりにしない。

- BANは再起動で消える。256件上限、満杯時拒否、16 slot/200msの掃除、有限TTLは変更しない。
- 自動復旧できるのは、記録・所有リンク・socketの照合と非生存確認に成功した場合だけ。
- 自動で次のdirectoryを生成したり、別名を探して再試行したりしない。手動の切替先も固定1か所に限定する。
- 本手順は証拠を保全したままサービス入口を復旧する手順。古い証拠の削除・廃棄は別の判断であり、本手順に含めない。
- 不明なXDP program/linkをdetachしない。別directoryを使っても、同じinterfaceに既存のXDPがあれば新daemonは装着を拒否する。

## 読取り専用の診断

```sh
sudo target/release/veil-local-ban inspect-runtime --control-dir /run/veil-local-ban
```

`inspect-runtime`は、既存の専用0700 directoryを開き、非待機でlock状態を確認する。固定6名のtype/UID/mode/device/inode/link数/sizeと、所有記録の整合性を表示する。
**bind、connect、unlink、rename、file書込みをしない。** socketをprobeしないので、診断によって他のlistenerへリクエストを送らない。symlinkをたどって記録やfile内容を読まない。
72-byte所有記録はowner/mode/type等の確認後にformat/checksumだけを検査し、本文を表示しない。未知のfileやdirectory全体を探索して内容を出す機能はない。

出力は`OK`ではなく`INSPECTED`で始まる。終了コード0は**診断の実行成功**であり、正常・回収可能・復旧完了の意味ではない。
読み出しによるfilesystemのatime更新まで禁止する意味の「無変更」ではなく、アプリからの書込み・修復操作がないという契約。

| field | 意味 / 次の判断 |
| --- | --- |
| `owner_lock=busy` | 別の所有者・起動処理があり得る。切替を進めず、その所有者を確認する |
| `owner_lock=available` | advisory lockを一時取得できた。socketや他processの死亡を証明しない |
| `owner_record=valid` | format/checksumの整合性だけ。socketの所有や死亡を保証しない |
| `owner_record=missing/invalid` | 自動回収の根拠がない。記録を新規作成・修復して迂回しない |
| `pending_record=present` | 初期化の残骸または未知のfile。自動削除しない |
| `socket_liveness=not_probed` | 診断では生存確認をしていない |
| `recovery_attempted=false` | 診断で復旧を実行していない |

稼働中はlockを奪わずmetadataのsnapshotだけを表示する。初期化中のsnapshotは変化し得るため、これだけを自動判断に使わない。
実装: [socket診断](../../veil-node/src/local_ban/sockets.rs)、[CLI](../../veil-node/src/local_ban/runtime.rs)。

## 初期化途中のSIGKILLをどう扱うか

次の表は、freshな空のcontrol directoryを使った隔離試験での結果。

| 停止点 | SIGKILL後の状態 | 通常の`serve`再実行 |
| --- | --- | --- |
| directory lease取得直後 | file/socketなし | 空で起動可能 |
| 通常socket作成直後 | socketのみ、記録なし | 保全して拒否 |
| 通常所有リンク作成後 | 通常socket+link、記録なし | 保全して拒否 |
| 緊急socket作成直後 | 緊急側link/記録が未完成 | 保全して拒否 |
| 緊急所有リンク作成後 | socket/link両組、記録なし | 保全して拒否 |
| 公開準備file作成後 | `.socket-owner.next`が0 bytes | 保全して拒否 |
| 公開準備file書込み後 | `.socket-owner.next`が72 bytes | 完成して見えても保全して拒否 |
| 所有記録公開後 | 記録と両socket/linkが一致 | 所有確認を経て空で起動可能 |
| XDP装着後 | 上記に加えXDP linkを所有 | SIGKILLでlink消失、空で再起動可能 |

記録が正しくても、所有リンクの欠損、socketの差替え、生きたlistenerがあれば回収しない。旧WALや過去のBANを復元しない。
既に残骸のあるdirectoryでのあらゆる命令位置、電源断、storage故障をこの9点で網羅したとは言わない。

## 手動復旧の手順

### 1. 対象を特定し、自動再起動を止める

導入先で承認した起動管理を使い、**対象daemonだけ**の自動再起動を停止する。生きている対象daemonは正常終了を優先する。
systemd運用を採用した場合は、[systemd運用手順](2026-10-24-LOCAL-BAN-SYSTEMD-OPERATIONS.md)の`veil-local-ban.service`を停止する。以下の直接起動例は従来のroot運用向けで、systemd運用の専用UID・`/var/lib/veil-local-ban/{primary,recovery}`とは区別する。未知のPIDをkillしない。

元のcontrol directoryと同じ実行UIDで診断する。以下はroot運用・host network namespaceの例。interface、宛先、port、保護IPは承認済み設定に合わせて決める。

```sh
VEIL_BIN=/absolute/path/to/veil-local-ban
VEIL_OLD_CONTROL=/run/veil-local-ban
VEIL_RECOVERY_CONTROL=/run/veil-local-ban-recovery
VEIL_IFACE=eth0

sudo "$VEIL_BIN" inspect-runtime --control-dir "$VEIL_OLD_CONTROL"
sudo ip -details link show dev "$VEIL_IFACE"
sudo ss -xlpn
```

namespace内のdaemonなら、XDPとprocess/socketの確認は**同じnamespace**で行う。host側に表示されないことだけで不在と判定しない。
`ss`のpath表示はbind時の`/proc/self/fd/...`名になる場合がある。実control pathの文字列検索だけで所有者不在を証明しない。

**ここで止める条件:**

- 対象daemonの停止を把握できない、`owner_lock=busy`、想定外のprocess/socketの所有者を特定できない。
- 対象interfaceにXDPが残り、その所有者・用途が不明。または別のXDPが稼働している。
- 実行UID、専用directory、interface、service scopeが承認済み設定と一致しない。
- 元directoryの内容を別processが並行変更している。

生きたsocketが無関係のprocessのものであると確認できた場合、それはそのまま保全する。切替先の管理入口とは別のものとして扱い、止めたり削除したりしない。

### 2. 元directoryを保全する

壊れた`socket-owner`や`.socket-owner.next`を編集・削除しない。socket、所有リンク、その他のfileもその場に残す。
元directoryをrenameして見えなくしたり、symlinkで切替先へ向けたりしない。記録のchecksumだけを再計算して正常扱いにしない。

診断結果・対象daemonの終了情報・XDPの状態を、組織で承認したprivateな記録先へ保存する。既存fileへ上書きせず、秘密鍵や無関係のfile本文を取り込まない。
この手順には自動archiveや削除コマンドを含めない。

### 3. 固定の切替先を、新しく空で作る

上の確認を終えてから実行する。**`-p`を付けず、既存なら失敗させる。** 既存の切替先に対してchmod・上書き・掃除をして通さない。

```sh
sudo mkdir -m 0700 -- "$VEIL_RECOVERY_CONTROL"
sudo "$VEIL_BIN" inspect-runtime --control-dir "$VEIL_RECOVERY_CONTROL"
```

新しいdirectoryでは、owner lockがavailable、所有記録とpendingがmissing、6つのpathがmissingであることを確認する。
root以外の専用UIDで運用する場合は、その承認済みUIDで新規作成する。既存directoryのownerを変更して流用しない。

切替先が既に存在する、またはそこも破損している場合は停止する。`recovery-2`、時刻付きdirectory等を自動的に増やさない。

### 4. 起動引数のcontrol directoryだけを切り替える

承認済みの同じbinary、object、interface、destination、port、保護IPで起動する。例のIPは文書用で、実環境の値ではない。

```sh
sudo "$VEIL_BIN" serve \
  --iface "$VEIL_IFACE" --ebpf-object /absolute/path/to/assets/veil_xdp_kern.o \
  --control-dir "$VEIL_RECOVERY_CONTROL" \
  --destination 203.0.113.10 --port 443 --protected-ip 198.51.100.10
```

既存XDPによって起動が拒否されたら、そのXDPを強制detachせず確認へ戻る。
この時も元directoryは変更しない。新しい入口への管理client・監視・起動管理の設定切替は、導入先で承認した変更として行う。daemonが自動で元の入口を置換することはない。

### 5. 復旧を確認する

```sh
sudo "$VEIL_BIN" status --control-dir "$VEIL_RECOVERY_CONTROL"
```

`mode=local-ephemeral`、`capacity=256`、`occupied=0`と新しいinstanceを確認する。以前のinstanceが分かっていれば違うことも確認する。
そのうえで、対象の入口を実際に通る通常HTTPS通信を確認する。Mapやstatusの表示だけを通信回復の証拠にしない。CDN等の別経路の成功を代用しない。

古いBAN命令・queueを再投入しない。再BANが必要なら、新しいinstance/revisionで管理者が対象と有限TTLを改めて判断する。
以後の通常再起動は同じ切替先を使う。元directoryへの復帰や証拠の廃棄は、調査と別の承認を経て行う。**手動切替は追加BANを復旧するものではない。** 基礎防御は別に維持する。

## 検証と証拠

### 単体・回帰

- Linux library: **20 passed / 0 failed / 1 ignored**。ignoredは下記SIGKILL試験で明示実行するsubprocess fixture。
- 既存BFT binaryの2並列回帰: **183 passed / 0 failed / 7 ignored / 3 filtered out**。TUI/既存root Linux proofの3件は明示除外。
- macOS portable engine: **7 passed / 0 failed**。Linux専用試験をmacOSの成功に含めない。
- 診断がlistenerへ接続せず、live leaseを奪わず、壊れた記録/pending/socketを変更しないことを単体検証した。
- 既存の容量・期限・解除・再送・Map異常の回帰も維持。新しい依存やWAL変更はない。

### pi-node: 初期化・破損・手動復旧

[専用script](../../scripts/local-ban-init-recovery-check.py)を追加し、**85項目が通過**した。
初期化の9停止点は、同じRustの`serve`/socket初期化処理を呼ぶ**library test実行体**を使用した。到達した停止点とprocessの停止状態を確認してからSIGKILLし、実際にreapして終了コードも確認する。
停止点のコードは`#[cfg(test)]`のみで、製品binaryへ停止機能や有効化featureを追加していない。
試験用実行体と製品binaryのhashを別に保存する。製品binaryを全命令位置でkillした検証と同一視しない。

初期化停止後の診断、起動拒否、自動復旧、手動切替は**製品binary**で確認した。製品binaryに試験用環境変数を渡しても停止しないことも確認している。

- 所有公開前の6つの残骸caseは、診断・通常起動拒否・手動切替後まで元の全entryを保全。
- 記録公開後/XDP装着後と、まだfileのないlease取得直後は、通常起動でBAN 0・HTTPS成功。
- checksum不正、17-byteへの切詰め、pending file残存も保全して拒否し、固定の空切替先でBAN 0・HTTPS成功。
- 別processである試験driverのlistenerと未知のfileを保全し、切替後もそのlistenerへ接続できる。
- 稼働中のdaemonの記録をfixtureとして破損させても、診断はlock busyを表示。同じdirectoryでの起動は拒否され、別directoryでの起動も既存XDPを置換できない。元のdaemon・socket・HTTP通信を保全。

保全比較は各entryのdevice/inode・type/mode・UID・link数・size・mtime/ctimeと、試験用通常fileの内容hashを使用した。atimeは比較対象外。破損データは自分で新規作成したprivateなfixtureだけを変更し、既存データへ障害を注入していない。

既存`ns-veil-rogue`の`veth-rogue`だけを用い、namespace内の`10.99.0.66:18443`へホスト`10.99.0.1`からTLS/HTTPで接続した。IP SANとrun固有bodyまで検証した。
新しい切替先と元directoryの両方を証拠として残し、試験process終了・XDP非装着を確認した。UFW/NTP/物理NICは変更していない。

### 既存の実機回帰

[既存受入れscript](../../scripts/local-ban-check.py)も同じ製品binaryで再実行し、**78項目が通過**した。通常HTTPS、scope外port、TTL、SIGSTOP中のkernel失効、個別解除、緊急bypass、256件/257件目拒否、SIGKILL後の同時復旧を確認した。
試験process終了・XDP非装着も確認。結果とsource保全確認は一次証拠に保存した。

### 一次証拠

- local: `artifacts/init-recovery-lb39zkrl/`
- pi-node: `/home/maakie/vlbi-XRl0adcE/`
- 初期化・手動復旧: `init-recovery-50nna8er/result.json`
- 既存実機回帰: `local-ban-sqhsogkq/result.json`
- 製品binary SHA-256: `9e1d6d48276e1c3ffd1cd089a72d36ececee7cca6c7b9e26eafba7e65e8c0093`
- fixture SHA-256: `831d7bdce12bf6780c5ac88e83c5f38456e4cbd5f815d9521febaff048103f07`

## 判定と残る限界

**今回の初期化途中SIGKILL・記録破損時の安全な拒否と、元を保全する手動切替は、隔離Linuxで確認した。**
この手動復旧は、未知のfileを消して自動復旧率を上げる実装ではない。問題の原因調査と証拠の整理は残る。専用directoryと切替先の両方を無制限に増やさない。

同一UID/rootがdirectoryを敵対的に並行変更する攻撃、storage故障/電源断、全初期化命令位置、実サイトの起動管理・権限設計・変更承認・実browser受入れは別の範囲。
最終ゴールの実サイト運用へは、導入先で停止条件と担当者を確定し、この手順を運用者が実行する受入れを残す。
