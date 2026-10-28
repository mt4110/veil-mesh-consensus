# ローカル一時BAN: 実装・隔離Linux受入れ

> 追記: 正常起動後のSIGKILLからの管理socket回収・再起動を追加し、隔離Linuxで確認しました。[新しい復旧契約・検証記録](2026-10-24-LOCAL-BAN-CRASH-RESTART.md)を参照してください。以下は前回時点の記録です。

2026-10-24。ユーザーが「再起動で一時BANが消える」契約を採用したため、[状態と掃除を制限する設計](2026-10-24-PUBLIC-SITE-BOUNDED-MINIMUM.md)の最小経路を実装した。
最終ゴールは、小規模公開サイトで安全に運用・解除できる無償OSS。今回のゴールは、既存BFTを保全して別のローカル一時BANを実装し、固定上限・期限・解除・再起動を隔離Linuxで確認すること。

## 実装した契約

専用binaryは **`veil-local-ban`**。従来の`veil-node`はそのまま残す。
新経路は署名鍵、membership、QUIC、投票、WAL、LSMを使わない。既存WALを読み替えず、移行・削除・期限延長をしない。旧BFTで必要な投票前永続化や`sync_all()`を省略する修正ではない。

- 1 Linuxホスト、指定した1宛先IPv4・TCP portを対象にする。既存C XDP objectと`PeerPolicy`のABI、kernel BOOTTIME期限、scope/counter Mapを流用する。
- BANは固定256 slot。期限切れで削除未確認のslot、挿入失敗で未確定のslotも含む。満杯時の新規IPは`CAPACITY`で拒否し、有効BANを追い出さない。既存IPの明示更新は可能。
- TTLは1秒以上24時間以下。CLOCK_BOOTTIMEにchecked加算して固定する。UTC/NTPを参照しない。非ゼロのtime namespace offsetや時計取得失敗を拒否する。
- 200msごとに16 slot検査・最大16 Map削除。cursor巡回、missed tickはSkip。削除失敗時はslotを保持し、後の巡回で再試行する。全件削除を一度に実行しない。
- **期限後はkernelがそのBANでDROPしない。掃除は空きslotを回収する処理。** daemonのSIGSTOP中も期限判定は動く。1巡3.2秒は予定どおり実行された場合の回収目安であり、OS/syscallの時間保証ではない。
- kernel HashMapの物理容量は既存4096。空Map・ABI・容量を検査して新しく所有する。agentが追跡・書き込むIPは256以下。物理容量を256へ減らしたと表現しない。
- 管理入口は所有者限定のUnix socket。既存0700 directoryとSO_PEERCREDのUIDを検証し、socketは0600。requestは固定40 bytes＋EOF、読取り2秒、応答も上限4096 bytes。通常8枠、独立した緊急枠1、listen backlogもそれぞれ8/1。
- BAN/解除は受付済み同時1操作、競合する追加変更の待ちqueueなし、200msに1変更まで。[systemd整備](2026-10-24-LOCAL-BAN-SYSTEMD-OPERATIONS.md)で、この1操作は掃除lockの解放を既存のjob期限2秒の範囲で待つようにした。Map操作中のロックとpermitは、呼出し元の応答待ちが時間切れになっても保持して追跡を失わない。自動再送しない。
- 管理命令は起動ごとのrandom instance IDとexpected revisionを必須にする。古いinstance/解除前のrevisionを拒否する。履歴を無限保存せず、直近診断イベントは固定64件のRAM ringにする。
- 緊急bypassは別socket・別revision・別Map lockを使う。通常BANの待ちqueueには入れない。緊急解除のためbypassは通常の5変更/秒制限と分けるが、同時1件。kernel syscall自体の停止時間を保証するものではない。
- kernel挿入失敗では成功を返さずslotを保持する。既存BANを短く更新する操作が失敗したときは、旧・新の長い方の期限まで不明状態を追跡する。旧kernel期限が残り得る間に`Pass`と誤表示しない。
- `APPLIED`はMap syscall成功の意味。`policy_mirror`はユーザー空間の見込みであり、通信観測の代用ではない。全体のkernel counterと実際の通信で確認する。
- XDPだけをロードしFD-owned linkを持つ。既存の装着を置換しない。SIGINT/SIGTERM正常終了で自分が作ったsocketだけをinode確認して回収し、linkを閉じる。終了/再起動で追加BANは消える。

コード: [engine](../../veil-node/src/local_ban/mod.rs)、[Linux adapter](../../veil-node/src/local_ban/kernel.rs)、[管理・runtime](../../veil-node/src/local_ban/runtime.rs)、[binary](../../veil-node/src/bin/veil-local-ban.rs)。
新しい直接依存は追加していない。`default-run = "veil-node"`を追加し、binary追加後も既存の`cargo run -p veil-node`を維持した。

## 手動操作の例

以下は導入先で値を決定した後の操作例であり、本作業で公開サービスへ配備した設定ではない。
指定IP/portを他サイトと共有しないこと、実送信元IPが入口で見えること、管理元IPを保護することを確認する。
Linux >=5.9と対象interfaceのXDP対応が必要。今回実測したのは7.0.0-38-generic / veth。

```sh
cargo build --release -p veil-node --bin veil-local-ban
sudo install -d -m 0700 /run/veil-local-ban
sudo target/release/veil-local-ban serve \
  --iface eth0 --ebpf-object assets/veil_xdp_kern.o \
  --control-dir /run/veil-local-ban \
  --destination 203.0.113.10 --port 443 \
  --protected-ip 198.51.100.10
```

別terminalでstatusからinstance/revisionを取得して明示的に操作する。例の203.0.113/198.51.100は文書用アドレス。BAN対象も実環境に合わせて決める。

```sh
STATUS=$(sudo target/release/veil-local-ban status --control-dir /run/veil-local-ban)
INSTANCE=$(printf '%s\n' "$STATUS" | sed -n 's/^instance=//p')
REVISION=$(printf '%s\n' "$STATUS" | sed -n 's/^revision=//p')
sudo target/release/veil-local-ban ban --control-dir /run/veil-local-ban \
  --ip 198.51.100.20 --ttl-secs 30 \
  --instance "$INSTANCE" --expected-revision "$REVISION"
```

解除は同じ形の`unban`で、最新statusのrevisionを指定する。競合/結果不明時はstatusを見直し、意図を確認して次の操作を決める。revisionだけ自動で取り直して再BANしない。
`bypass --mode on|off`もinstanceを必須にし、**`control_revision`**をexpected revisionに渡す。通常BAN用revisionとは別。

SIGKILL等の異常終了ではsocketが残り得る。起動はそれを勝手にunlinkせず拒否する。残存process/link/pathの所有関係を確認してから運用者が回収する必要がある。
正常終了を使う今回の再起動試験と、あらゆる異常終了後の無人再起動を同じ保証にしない。古いsocketを無制限に別名で迂回しない。

## 検証方法と確認範囲

### 単体・回帰

- macOS: portable engine **7 passed**。実kernel装着をmacOS試験と同一視しない。
- pi-node Linux: 新library **9 passed**。固定frame/余分なbyteの拒否、256件容量、全件満了時の16件掃除、insert/delete失敗、短縮更新失敗、rate/保護IP/TTL、古いrevision、overflowを確認。
- 同じsourceで既存binaryの回帰を2並列実行: **183 passed / 0 failed / 7 ignored / 3 filtered out**。投票前保存、二段階証明、再起動、checkpoint、期限を含む。TUI・既存root Linux proofの3件は、この回帰では明示除外。今回の実kernel受入れは別scriptで行った。
- Linux buildは1.98.1のnative rustc、vendored依存、専用home配下のTMPDIR/targetを使った。旧build成果物を上書きしない。

### pi-nodeで実際のHTTPS/XDP

再実行可能な[隔離受入れscript](../../scripts/local-ban-check.py)を追加した。
既存`ns-veil-rogue`の`veth-rogue`へXDPを装着し、**namespace内の10.99.0.66:18443へホスト側10.99.0.1からTCP/TLS/HTTPで接続**する。証明書のIP SANを検証し、HTTP bodyのrun固有値まで照合する。
対象外の18444にもHTTPS serverを置き、scopeが広がらないことを確認する。ブラウザ実機操作や実サイトの受入れ試験ではない。

確認した振る舞い:

1. 装着前/空MapでHTTPS成功、BAN後は対象portの通信失敗とkernel DROP counter増加。対象外portは成功。
2. 3秒TTLのBAN中にdaemonをSIGSTOPしても、期限後はHTTPSが成功。SIGCONT後の掃除は16件以下。
3. 明示解除でHTTPS回復。解除前のBAN命令を再送しても拒否され、再遮断しない。
4. 通常socketに8 slow readerを置いた状態でも緊急bypassが使え、HTTPS回復。bypassを戻すと未失効BANが再び作用。
5. SIGTERM停止で所有linkとsocketが消える。新しいinstanceで空Mapから起動し通信成功。古いinstanceの命令は拒否。
6. 通常の5変更/秒以下の経路で256件を投入。257件目拒否、既存BAN維持、対象外HTTPS通過、ring64件上限。満杯からの再起動も空で通信成功。
7. 同じinterfaceへ二重起動すると拒否され、先にあるXDP linkを保全。管理socket名に既存fileがあっても保全して起動拒否。
8. 最後に自分のdaemon/serverのみ終了し、XDP非装着・子process終了を確認。既存namespace/vethは残す。UFW/NTP/物理NICは変更しない。

最初の試験には、scriptの関数名衝突とXDP flag指定による失敗があった。失敗artifactも保存した。
`BPF_LINK_CREATE`経路へnetlink用`UPDATE_IF_NOEXIST`を指定していたためEINVALになった。Aya 0.13.1と[Linux v7.0のdev_xdp_attach](https://kernel.googlesource.com/pub/scm/linux/kernel/git/torvalds/linux/+/refs/tags/v7.0/net/core/dev.c)を照合し、link経路で許可されたflagに修正した。
既存program/linkはkernel側で原子的に拒否されることを確認し、二重起動試験でも既存linkが保全されることを実測した。上書きfallbackは追加していない。

一次証拠はlocal `artifacts/local-ban-uv9ntiwf/`とpi-node `/home/maakie/vlb-ueFAdEcI/`に保存。source、実行binary/objectのhash、回帰stdout、HTTPS結果、失敗記録を残す。試験用private keyはprivateなremote artifact内だけに置き、reportやソースへコピーしない。
開始時97 fileをhash化・archiveした。既存BFTソース、WAL保存実装、C/Rust XDP実装、既存object、Cargo.lockは今回変更していない。既存WAL/署名鍵の移行や削除は行っていない。旧source全体と既存dirty workをGitのHEADへ戻す処理も行っていない。

## 判定と残る境界

**上限付きローカル一時BANの実装・隔離Linux受入れは完了。用途に合う導入先での限定試験へ進む候補として採用する。公開サービスへ直ちに配備可とまでは判定しない。**
これはBFTの3秒合意性能を改善した結果ではなく、保存/分散合意を必要としない別契約の実装。従来のBFT試験やTTL/安全条件は変更していない。

- 再起動で防御が消える。攻撃継続中の停止/再起動では、既存HTTPS/firewall/HTTP制限が基礎防御になる。
- 新経路はローカル所有者による手動操作だけ。自動検知、他hostへのBAN配布、durable監査、永続BAN、remote controlを実装していない。
- CDN/proxyのIPを利用者の代わりにBANしない。IPv6、UDP/HTTP3、VLAN、fragmentはこの追加制御の対象外。IP/port共有サイト、同一NATの利用者も区別できない。
- 256 IPを超える分散攻撃には満杯拒否で追加防御の限界が来る。回線飽和をhost内XDPだけでは解決できない。
- 件数と掃除の仕事量を制限した。総RSS/kernelメモリの厳密な上限、kernel syscallの最大時間、HTTPアプリの状態量まで保証したとは言わない。未知IPのpacketから新規BANを生成する経路はない。
- SIGKILL後のsocket回収・異なるNIC/kernel/実browserでの運用受入れは未検証。実サイト構成、基礎防御、管理元IP、緊急復旧担当を決めてから限定試験に進む。

実行設定: Sol / High。単独担当、追加agentなし。実モデル/使用量の確認範囲は最終報告に記載する。
