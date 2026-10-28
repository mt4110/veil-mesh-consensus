# ローカル一時BAN: SIGKILL後の管理ソケット回収と再起動

> 追記: 初期化途中のSIGKILL・所有記録破損時の拒否と、元を変更しない手動復旧を検証しました。[診断と手動復旧の手順](2026-10-24-LOCAL-BAN-MANUAL-RECOVERY.md)を参照してください。以下は前回時点の記録です。

2026-10-24。[前回の一時BAN実装](2026-10-24-LOCAL-EPHEMERAL-BAN-IMPLEMENTATION.md)で残したSIGKILL後の復旧を追加した。
最終ゴールは、小規模公開サイトで安全に運用・復旧できる無償OSS。今回の完了条件は、所有を確認した管理ソケットだけを回収し、既存データ・他の所有者を保全して、空のBAN状態で再起動することを回帰と隔離Linuxで確認すること。

## 結論

**正常に起動した新版daemonがSIGKILLされた場合、同じ`serve`コマンドで、自分の残した管理ソケットを確認・回収して再起動できる。再起動後のBANは空。**
256件上限、満杯拒否、分割掃除、TTL、管理接続数、既存BFT/WALは変更していない。

所有を確認できない場合は、削除して復旧を強行せず、理由を返して起動を拒否する。正常起動後のSIGKILL復旧と、どの初期化命令で殺しても必ず自動復旧するという保証は区別する。

## 回収を許可する条件

[専用のsocket管理module](../../veil-node/src/local_ban/sockets.rs)を追加した。

1. 既存の専用0700 directoryを`O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`で開き、owner UIDとmodeを確認する。そのdirectoryのFDに非待機の排他`flock`を取り、終了まで保持する。同時に起動・回収できるdaemonは1つだけ。
2. file操作は`/proc/self/fd/<directory-fd>/固定名`に固定する。親のpathが移動しても、別のdirectoryを削除対象にしない。
3. 自分が作った通常/緊急socketにそれぞれ所有確認用hard linkを置く。元と同じinodeを保持するので、残った数字のinode番号が別のsocketに再利用されたことを所有の根拠にしない。
4. owner-onlyの固定72-byte記録`socket-owner`に、2つのdevice/inode番号とformat識別子・SHA-256 checksumを保存する。BAN対象、TTL、revision、投票、鍵は保存しない。これは署名ではなく整合性検査であり、権限の根拠は専用directory・UID・所有リンク・排他制御にある。
5. 再起動時は、記録のformat/size/owner/mode/link数、各socketのtype/owner/mode、記録とhard linkとのinode一致、公開socket名があればその一致も確認する。通常・緊急の**両方を検査してから**回収する。
6. 所有リンク経由でnonblocking connectを試し、**ECONNREFUSEDだけ**を回収可とする。接続成功、backlog満杯、権限異常等の不明な結果は拒否する。lockが解放されていても、リスナーを持つ別プロセスがいれば保全する。
7. unlink直前にidentityを再確認し、公開名→所有リンクの順で自分のsocketだけを回収する。公開名を回収したところで中断されても、記録と所有リンクが有効なら次回続行できる。
8. 新socketと所有リンクを作り、新記録を固定名のtemporary fileから原子的renameで公開する。その後に既存のXDP専用経路へ進む。新しいMap・instance ID・revisionで開始し、BANを復元しない。

`flock`はPIDではなくopen file descriptionに結びつき、全FDが閉じると解放される。PID再利用や、何秒待てば死んだとみなすか、という判定を使わない。[Linux flock(2)](https://man7.org/linux/man-pages/man2/flock.2.html)
hard linkは同じfileを指す別名であり、inodeを保持する。[Linux link(2)](https://man7.org/linux/man-pages/man2/link.2.html)
pathname Unix socketの性質・エラーは[Linux unix(7)](https://man7.org/linux/man-pages/man7/unix.7.html)と実機を照合した。

## 状態と掃除の上限は維持

| 名前 | 用途 |
| --- | --- |
| `control.sock` / `emergency.sock` | 従来どおりの管理入口2つ |
| `.control.owned.sock` / `.emergency.owned.sock` | 上記と同じinodeを保持する所有確認用リンク2つ |
| `socket-owner` | 固定72 bytesの所有記録1つ |
| `.socket-owner.next` | 起動中の公開準備用。最大72 bytes、固定名1つ |

daemonが管理する名前は最大6つで、起動回数に比例したdirectory・履歴・tombstoneを作らない。
正常停止ではsocketの4名を回収し、72-byteの所有記録だけを残す。次回起動で更新する。
これらの所有確認用リンクも0600かつ0700 directory内で、管理者以外に別の入口を開くものではない。

所有記録の書込みは**起動時だけ**。BAN/解除/16件ずつの掃除にfile書込み・fsync・renameを追加していない。BANの保存方式やWALを新設したものではない。
SIGKILLからの復旧はkernel内に残ったfile状態を使い、追加fsyncは行わない。storage/host障害で記録が不整合なら保全して拒否し、正しい記録が必ずdurableに残るとは保証しない。

起動時の検査・回収はsocket 2組だけ。接続待ちや無制限の探索・再試行はしない。filesystem/syscall全体の最大所要時間を保証したものではない。

## 拒否して保全するもの

- 所有記録のないsocket。**UIDが同じで、すでに死んだsocketでも自動採用しない。** 旧版をSIGKILLした残骸も該当する。
- 稼働中の別daemon、lockがなくてもまだlistenするsocket、見知らぬ通常file/symlink。
- checksum不正、size/mode不正、symlink/hard link化された所有記録、公開準備中の記録が残った状態。
- 所有リンクの欠損、inode不一致、別socketへの差替え。片方の検査が失敗した場合、もう片方も勝手に回収しない。

所有記録公開前などの初期化中にSIGKILLすると、所有を証明できないpathや`.socket-owner.next`が残り、自動復旧を拒否し得る。その場合は、process・link・pathの所有と状態を個別に確認する必要がある。**強制削除flagや、未知のfileを消すfallbackは実装していない。**
旧版が生きている場合は、旧版を正常終了してsocketが回収された後、新版を起動する。所有記録のない古い残骸を今回の試験で移行・削除してはいない。

Linuxのlocal filesystem・procfsを前提とする。`flock`はadvisoryであり、同一UID/rootが意図的にdirectory内容を書き換える攻撃まで隔離する仕組みではない。
専用directoryを共有作業領域にせず、同じUIDの別プログラムがその内部を並行変更しない運用を前提とする。root侵害への耐性やネットワークfilesystem対応とは表現しない。

## 回帰とpi-node受入れ

### 自動検証

- Linux library: **18 passed / 0 failed**。前回の9件に加えて、所有記録による回収、directory lease、CLOEXEC、lockなしのlive listener保全、未知のlive/dead socket・通常file・symlink保全、片方の差替え、途中までの回収、破損記録、失われた所有リンク、親path差替えを確認した。
- 既存BFT binaryを2並列: **183 passed / 0 failed / 7 ignored / 3 filtered out**。TUIと既存root Linux proofの3件は明示除外。実kernelの確認は以下の独立scriptで実施した。
- macOS portable engine: **7 passed / 0 failed**。Linux専用のsocket復旧試験をmacOSの成功に数えていない。
- rustfmt check、Python構文検査を実施した。新しい依存は追加していない。

### 実プロセス・HTTPS・XDP

[隔離受入れscript](../../scripts/local-ban-check.py)を拡張。pi-nodeの既存`ns-veil-rogue` / `veth-rogue`だけを用い、namespace内のHTTPS `10.99.0.66:18443`へホスト側`10.99.0.1`から接続する。証明書のIP SANとrun固有HTTP bodyを確認する。

**78項目が通過し、試験用daemon/serverの終了とXDP非装着も確認した。** 主な追加確認:

1. 稼働中に同じcontrol directoryで二重起動すると`CONTROL_DIRECTORY_BUSY`で拒否し、socket inodeとinstanceを保全。
2. 別プロセスである試験driverが保持する無関係のlistenerを保全。listener終了後の未知のstale socketも消さずに拒否。
3. **256件の有効BANを持つdaemonをSIGKILL**。実際にreapして終了コードを確認し、XDP link消失・HTTPS回復・管理socketと所有リンクの残存を確認。
4. その状態で**2つの復旧プロセスを同時起動**。1つだけがREADYになり、他方はdirectory lockで拒否された。成功側は容量256・使用0の空状態、instanceは変更、HTTPS成功。
5. SIGKILL前のinstanceによるBANは拒否。復旧後の新しいBANでHTTPSが遮断され、明示解除で再度回復。
6. 再びBAN→SIGKILL→同じdirectoryで再起動して、空状態・HTTPS成功を確認。正常終了後は`socket-owner`の72 bytesだけが残る。

前回の正常通信・対象外port・TTL・SIGSTOP中のkernel期限失効・個別解除・slow reader中の緊急bypass・257件目拒否も同じscriptで通過した。保護条件、TTL、既存の時間制限を緩めていない。
ブラウザの実操作・公開サイト・NIC違いの試験と同一視しない。

一次証拠:

- local: `artifacts/socket-recovery-w7cbhcoa/`
- pi-node: `/home/maakie/vlbs-8jXTaGpx/`
- 実機結果: `local-ban-q892kyo2/result.json`
- 実行binary SHA-256: `1342d842aa9a2c2d27ca23f7e75640d8ff1ea2be4a1bcd253f8fc8756f283e99`

旧試験・旧build directoryは保全し、fresh source/target/TMPDIRで検証した。UFW・NTP・物理NIC・既存namespace構成を変更していない。無関係のfixture socket/fileは証拠として残し、動いている試験用processだけを終了した。

## 採否

**正常起動後のSIGKILLからの安全な再起動は、限定試験用として採用する。今回の実装・隔離受入れのゴールは達成。**
実サイトで運用できる無償OSSという最終ゴールは継続する。再起動で追加防御が消える契約、実送信元が見える専用IPv4/TCP入口という制約、基礎防御と導入先での運用受入れは維持する。
所有を証明できない起動途中の残骸は「安全に拒否」する境界として残る。自動回収率を上げるために、未知のfile/socketの保全を弱める変更は行っていない。
