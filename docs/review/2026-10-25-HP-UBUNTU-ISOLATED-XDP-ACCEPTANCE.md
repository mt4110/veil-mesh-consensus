# hp-ubuntuの起動確認と隔離XDP受入れ

2026-10-25 JST。Sol／High、分担なし。
**hp-ubuntuで既存の検証済み成果物を実XDPへ接続し、正常通信・遮断・解除・期限失効・256件上限・daemon再起動を確認した。一回の通し実行が成功し、終了後の停止・XDP解除・既存環境の前後比較も合格した。公開siteと物理NICの受入れは未実施。**

最終ゴールは、普通のブラウザを使う公開サイトで安全に試用・復旧できる無償OSS。
今回は新しいUbuntu hostの隔離受入れがゴール。BFT、WAL、製品コード、TTL、容量、操作間隔、安全条件は変更しない。

## 先に実施した一回のホスト再起動

当初はEFI変更・再起動を保留していたが、ユーザーがBitLocker回復キーの確保と、起動順変更・正常再起動一回を明示承認した。
その範囲で変更前を`/root/hp-ubuntu-boot-20261025T095753Z/`へ保存し、`BootOrder`を`0003,0001,0000,0002,0006,0005,0004`に変更。直後の読み戻しでは一致し、既存の起動項目も一致していた。
更新処理のpackage lockがなく、`dpkg --audit`と`sshd -t`で問題が出ないことを確認してから、`systemctl reboot`を一度だけ要求した。

- 再起動前boot ID：`87c5096a-4d7d-45cb-be1d-7d691b214864`。
- 再起動後boot ID：`8f4bc0d8-b69b-447a-b3e5-0c2895932039`。
- 新しいbootで`ssh hp-ubuntu`、鍵認証、`sudo -n true`が成功。同じDHCP IPv4 `192.168.1.6`を取得。SSH socketの自動待受が有効で、failed unitはなかった。
- 再起動後の数値順序は`0001,0000,0002,0006,0005,0004`。要求した数値順序は保持されていない。
- ただし`Boot0001`自体が、変更前の汎用ELECOM USB項目から、同じUSB ESPの`Ubuntu / \EFI\ubuntu\shimx64.efi`を指す項目へ変化しており、`BootCurrent=0001`。現在はUbuntuが先頭。項目の変化が発生した主体・時点の原因は未特定。
- 番号だけで変更前のELECOM項目のままとした途中説明は訂正した。数値順序の不一致を隠さず、実際のentry pathとSSH復帰を区別して記録する。

この一回はUbuntuへ戻った実測。追加の再起動、外付けdiskを外した場合、将来の更新後まで保証するものではない。
Windows用のディスクやEFI fileを編集・mountしていない。Windowsの実起動は未確認。回復キーの内容は受け取っていない。
隔離XDP試験の保全比較は、この再起動後をbaselineとしている。

## 固定した成果物と隔離範囲

| 対象 | 実体・条件 |
| --- | --- |
| host | `hp-ubuntu`、Ubuntu Server 26.04.1 LTS、Linux `7.0.0-38-generic`、x86_64、systemd 259 |
| binary | pi-nodeの検証済みdebug binaryを複製。SHA-256 `e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890` |
| C XDP object | SHA-256 `2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f` |
| source | 前回のmanifestにある73fileと現在のsourceのSHA-256が全件一致。hp-ubuntuでの再buildはしていない |
| 一時配置 | `/run/vhp-795ystyt/`。既存のbinary・配置物を上書きしていない |
| runner | 一時`vhp-795ystyt-runner.service`、`PrivateNetwork=yes`。hostのnet namespaceならdriverは開始を拒否する |
| server | runnerの子が作る別の匿名network namespace。試験終了時に所有processを終了して解放 |
| network | 上記二つのnamespace間だけのveth。client `10.99.0.1`、server `10.99.0.66`。host NIC・host routeへの接続なし |
| scope | `10.99.0.66:18443/TCP`。対象外の18444/TCPで通信継続も確認 |
| daemon | 一時`vhp-795ystyt-daemon.service`。server namespaceへ入れ、runnerに`BindsTo`を設定 |
| 実行user | systemd DynamicUser。観測した全三世代はUID/GID63452。常設accountの追加なし |

起動・停止・再起動・hardeningは既存の`deploy/systemd/veil-local-ban.service`から引き継いだ。初回の新規control directoryのみ、起動前に割り当てた一時UIDへ所有権を渡した。
起動後は全threadで非root、Eff／Prm／Inh／Ambゼロ、NoNewPrivs=1を確認。実行中のXDP program ID76、`veil_xdp_filter`の装着と、hostから分離したnamespaceも別の読取りで確認した。

DynamicUserは実行時だけuser/groupを割り当てる機能で、静的account DBを追加しない。[systemdの説明](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html#DynamicUser=)
今回も`/etc/passwd`・`/etc/group`の前後hashが一致。終了後は一時userの解放を確認し、再利用されるUIDが残ったfixtureへ入れないようcontrolの親をroot所有0700へ戻した。`socket-owner`は保全している。

パッケージ導入、Rust toolchain導入、常設unit、enable、sysctl、host firewall、物理NICへの装着は行っていない。通常のsystemd／journal logは発生する。

## 確認した振る舞い

既存のoperator-kernel試験の機能確認列を、隔離linkの参照先以外は同じ内容で使用した。
試験用証明書のIP SANとrun固有bodyを検証し、CLI応答だけで通信結果を代用していない。

| 確認 | 実測結果 |
| --- | --- |
| 正常通信 | 装着前、空BAN、解除後、期限失効後、restart後、最終停止後のHTTPSが成功 |
| preview | 件数・revision・event countが変わらず、HTTPSが継続 |
| 遮断 | previewのcommandを別の明示操作として適用。送信元の対象HTTPSを遮断し、kernel drop counterが増加。18444は継続 |
| 解除 | 新しいpreviewから解除し、HTTPS復帰と一覧からの除去を確認 |
| 期限 | TTL3秒。まだ有効なBANを確認してdaemonをSIGSTOPし、停止中でもkernel期限でHTTPS復帰 |
| 分割掃除 | SIGCONT後の既存4秒観測内にoccupied0・一覧0。max_examined／max_attemptedは16以下 |
| 256件 | TTL180秒、既存の操作間隔0.21秒で一件ずつ明示適用。実送信元を含む256件がactive／confirmed。全一覧26,501byteを取得 |
| 257件目 | preview・直接BANをどちらもCAPACITYで拒否。既存IP・期限・revisionを維持し、追い出しなし。対象の遮断と対象外通信も維持 |
| 通常daemon再起動 | 256件の状態からrestart。新instance、一覧0件、occupied0、HTTPS復帰 |
| SIGKILL | 改めてBANして遮断後mainをSIGKILL。既存のRestart条件で起動し、新instance・空BAN・HTTPS復帰 |
| 古い操作 | 古いrevisionの解除と解除済みBAN再送を拒否。再起動前instanceのBAN・解除も拒否 |
| 不正入力 | 保護IPとTTL0をpreviewで拒否し、実行commandを生成しない |
| 停止 | daemon停止後のXDP非装着とHTTPS復帰を確認してから、試験serverも終了 |

一回の通し実行で32確認が成功。CLI574回、変更命令267回。件数は入力・拒否・記録の追跡用で、品質や運用適合の証明そのものではない。
変更命令の自動再送、instance／revisionの書換え、TTLや時間制限の緩和はしていない。
結果不明の応答は今回発生していない。Library回帰は今回再実行しておらず、同じsourceの前回の記録と区別する。

## 終了後の独立した確認

- runnerとdaemonはともに`not-found / inactive / MainPID=0`。常設unit file・自動起動symlinkなし。一時unitをdisabledへ変更したという意味ではない。
- 試験pathをargvに持つprocessなし。serverも回収済み。停止前にserver側vethのXDP解除と通信復帰を確認し、終了後のhost BPF link一覧も開始前と一致。
- controlはroot:root／0700、残るentryは`socket-owner`だけ。未知のfileを削除・修復していない。
- SSH・networkd・resolved・timesyncdの観測した状態・MainPID・unit pathが前後一致。
- passwd／group／sudoers／承認済みsudo追加file／sshd_config／GRUB設定のmetadata・hashが一致。
- `/etc/netplan`と`/etc/systemd/system`のentry metadataが一致。host link、IPv4／IPv6 address・route、nft／iptables／ip6tables、EFI、boot IDも一致。
- IPv6の期限等の動的値のみ比較時に正規化。raw before／afterを保全しており、network構成全体を比較から外していない。
- 製品source73fileが終了後も前回manifestと一致。

これは対象を絞った前後比較であり、hostの全dataに対する完全な整合性証明ではない。
新規配置と証拠は`/run`に保持した。手動削除はしていない。次のhost再起動で失われるので、秘密鍵を除いた証拠をlocalへ取得済み。

## 証拠と次の段階

- [集約と保全比較](../../artifacts/hp-ubuntu-xdp-795ystyt/acceptance-summary.json)
- [全CLI・機能確認](../../artifacts/hp-ubuntu-xdp-795ystyt/result.json)
- [独立した終了監査・起動記録](../../artifacts/hp-ubuntu-xdp-795ystyt/independent-final-audit.json)
- [実行中のXDP装着](../../artifacts/hp-ubuntu-xdp-795ystyt/live-xdp.json)
- [試験driver](../../artifacts/hp-ubuntu-xdp-795ystyt/driver.py)、[前後監査](../../artifacts/hp-ubuntu-xdp-795ystyt/audit.py)、[実行log](../../artifacts/hp-ubuntu-xdp-795ystyt/execution.log)

今回の隔離受入れは完了。公開pilotはHOLDを維持する。
次は物理`eno1`のdriver・既存XDP/TC、実入口・保護IP・LANの所有client・復旧経路を読み取りで確定し、影響と復旧方法を承認してからLAN一件試験へ進む。
veth成功を物理NIC、IPv6／HTTP3、公開経路、独立client、本人の実操作、無人防御の合格に転記しない。
