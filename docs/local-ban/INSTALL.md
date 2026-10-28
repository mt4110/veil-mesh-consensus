# ローカル一時BANの導入

[README](../../README.md) · [操作](OPERATIONS.md) · [復旧](RECOVERY.md) · [試験前チェック](PILOT_CHECKLIST.md)

これはレビュー可能な配置・受入れ手順です。記載しただけで変更が承認されたことにはなりません。host／NIC／disk／設定値が未確定なら配置・装着しません。初回は停止・非enabledで終えます。

## 1. 導入に適合するか確認する

- 単一Linux host、plain Ethernetの入口、一つの宛先IPv4・TCP port。実client sourceが見えること。
- HTTPS終端・firewall・HTTP側制限が基礎防御として存在すること。単なる手動IP遮断で足りない要件をこの機能に押し込まない。
- root侵害、無人検知、全protocol遮断、回線飽和対策、URL／user単位制御は対象外。
- Linux >=5.9のFD所有XDP pathをコードで要求する。動作確認済みのkernel／vethと、導入先の物理NIC／driver対応は別。既存XDPとの共存・置換やfallbackを仮定しない。
- 専用の単純なEthernet NICで、LRO（rx-lro）とhardware GRO（rx-gro-hw）が**active／wantedとも事前にoff**であること。新loaderはdefault／generic両方で検査し、on・不明なら装着前に拒否する。既存XDP、master／下位device、未知の構成、clsact／ingress等のTC hookも拒否する。別管理者による同時設定変更を止められない共用NICには適用しない。
- CDN／別hostのL7 proxyでは、利用者の代わりに中継元をBANしない。適合しなければCDN／HTTP側で制御し、この追加profileは採用しない。
- 管理consoleと独立した復旧経路を確保する。dual bootではWindows起動中にsiteが止まるので、初回は時間を区切った有人試験。

[チェックリストの対象票](PILOT_CHECKLIST.md)で実入口・管理source・既存XDP・対象外通信を確定します。IPとportを他の独立siteと共有すると、そのsiteも遮断に巻き込みます。

## 2. 成果物を固定する

導入にはsource版、binary、C XDP objectの出所とhashを記録します。最新版Releaseという名前だけでは選びません。
最新の実XDP記録はLinux x86_64のdebug binary SHA-256 `e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890`と、C object SHA-256 `2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f`の組合せです。配布済みReleaseという意味ではありません。

新しい作業領域と必要なRust／C toolchainを用意した後のbuild例です。初期tool導入や既存targetの上書きは別途確認します。

```sh
cargo build --locked --release -p veil-node --bin veil-local-ban
cargo test --locked -p veil-node --lib
```

前回のnative buildはRust 1.98.1系列。offline vendor／Nix copyは当該pi-node用の復旧手段であり、他hostの必須構成ではありません。依存が既に揃っている場合は`--offline`も使えます。
新しいrelease buildや別architectureのbinaryは新hashになるため、以前のdebug binaryの受入れを転記せず、その実体で再確認します。
macOSでのlibrary成功はportableロジックのみ。実行binaryはLinux用です。

**2026-10-27のguard付きsourceは別成果物です。** [明示Generic・active／wanted guardの非課金検証](../review/2026-10-27-GENERIC-OFFLOAD-GUARD-IMPLEMENTATION.md)はLinux ARM64でのbuild・単体検証まで。上記x86_64 binaryの実XDP合格を、新loaderへ転記しません。試験packageの旧binary／manifestも保全しており、新sourceを自動的に含むpackageではありません。

起動時の`serve --xdp-mode default`は従来のflags0、`--xdp-mode generic`はSKB_MODEを明示します。省略はdefault。どちらも一回のFD所有link装着で、失敗時の自動fallback・既存program置換・pinはありません。標準unitでは`VEIL_XDP_MODE=default`を既定とし、承認した専用環境だけでconfigをgenericへ指定します。復旧用overrideも同じ値を使用します。記載はhost設定変更の承認ではありません。

装着前のethtoolはGSSET_INFO／GSTRINGS／GFEATURESの読取りだけです。requestedをwantedとして扱い、取得できない状態をoffと推測しません。qdiscは最大16個の単純構成に限り、許容qdiscでも全chain／priorityのTC filterをGETして、存在・取得失敗なら拒否します。装着後は同じifindex・name・MTU・単純構成、実modeとprogram ID、feature全体のactive／wanted、許容qdiscの構造一致とfilter不在を確認してからREADYとします。差分・取得失敗なら所有linkを解放して停止し、未知のNIC設定を自動復元しません。preflightはdefaultも含むため、従来起動できた一部構成も拒否する互換性変更です。

Generic装着自体がkernelでoffload設定を更新し得ます。アプリにSETがなくても装着は承認対象です。GET→装着は他管理者の変更に対して原子的ではなく、任意driverの全設定不変を保証する仕組みではありません。TCの確認も許容qdisc構成の範囲であり、未知の全kernel hookを列挙した保証ではありません。

loaderが確認したobjectは`veil-ebpf/src/veil_xdp_kern.c`に対応する`assets/veil_xdp_kern.o`。別のRust eBPF build出力を同じABIの検証済みobjectと仮定しません。objectを再build／変更する場合は新しい成果物として固定し直します。

## 3. 配置案と復旧方法を承認する

以下は標準unitの予定pathです。既存物があれば自動上書き・再帰chownをせず、用途と所有を調べて停止します。

| 対象 | 配置案／条件 | 戻し方 |
| --- | --- | --- |
| account | 専用`veil-local-ban` user／group。nologin、不要な補助groupなし。既存同名accountは流用前に確認 | 停止後も保全。account削除はこの手順に含めない |
| binary | `/usr/libexec/veil-local-ban`、root所有、service UIDが書換え不可。setuid／file capabilityなし | 新unitを止め、配置物は保全。旧file交換が必要なら別の退避・復元案を承認 |
| object | `/usr/lib/veil-local-ban/veil_xdp_kern.o`、root所有、service UIDが書換え不可 | 同上 |
| config | `/etc/veil-local-ban/service.conf`、root所有。実NIC・宛先・port・保護sourceを記入 | 今回変更した差分だけ戻す。未知の既存fileを上書きしない |
| state親 | `/var/lib/veil-local-ban`、新規root所有0755 | 削除せず保持 |
| control | `/var/lib/veil-local-ban/primary`、新規service UID所有0700 | 所有記録を保全。破損時は[固定先への復旧](RECOVERY.md) |
| unit | `/etc/systemd/system/veil-local-ban.service`。初回はenableしない | 対象unitを停止。必要なら承認済み新設fileだけ上書きなしで退避しdaemon-reload |

配布候補は[unit](../../deploy/systemd/veil-local-ban.service)、[設定例](../../deploy/systemd/veil-local-ban.service.conf.example)、[sysusers定義](../../deploy/sysusers.d/veil-local-ban.conf)。installerではなく、未配置のtemplateです。
sysusersによるaccount DB更新、directory作成と新規childへの所有権付与、配置先、daemon-reload、装着・停止、必要時の復旧差分まで具体化して承認します。既存account DBを丸ごと巻き戻す方法は採りません。
directory作成は既存なら失敗させ、既存物のchmod／chownで通さない方式にします。固定復旧先は`/var/lib/veil-local-ban/recovery`一か所。未知の既存recoveryも掃除して流用しません。

`VEIL_PROTECTED_IP`は**BAN禁止の送信元IPv4一件**で、守る宛先やCIDRではありません。宛先自身も自動除外されます。追加例外はCLIで最大255件までですが、標準unit例は一件です。複数必要なら`--protected-ip`を繰り返すunit案をレビューし、envへ空白区切りで詰め込みません。

## 4. 専用UID・unit・復旧先の整合を確認する

serviceは起動時にCAP_BPF／CAP_NET_ADMIN／CAP_PERFMONを使い、READY前に全threadのEff／Prm／Inh／Ambをゼロへ落とします。bounding setがゼロという意味ではありません。root／SYS_ADMINへのfallbackはしません。
標準unitは`Type=notify`、`Restart=on-abnormal`、待ち2秒、起動rate制限60秒に3回、start／stop上限10秒。条件を緩めて導入先を合格させません。
unitのactive／READYは初期化完了であり、HTTPS疎通や正しいtupleを証明しません。

実機に配置したunit・drop-inを構文検査し、User／Group、ExecStart、EnvironmentFile、ReadOnlyPaths／ReadWritePaths、binary／objectの実pathとhashを照合します。ここでは未確定hostへ設定適用しません。
`RuntimeDirectory`／`StateDirectory`やtmpfilesで未知の所有記録を削除・再帰chownする設定を追加しません。
NICの存在・実IP・route・基礎Web・保護sourceはstart前に確認します。`After=network.target`だけでアドレス準備完了とは判定しません。

## 5. 段階的に受け入れる

承認後の配置順は、専用accountの新設確認→新規directoryと所有権→固定したbinary／object→実値を入れたconfig／unit→unit構文と実効設定の確認→daemon-reloadです。sysusersを使う場合も、定義の配置先とaccount DB変更を先に承認します。

1. 選択したbinary／objectそのものを、新hostの隔離環境で確認する。通信、256／257件、期限、解除、正常／SIGKILL restart、旧操作拒否、権限破棄、最終停止を含める。
2. 承認したLANの物理入口で空BAN装着→一件BAN→解除／失効→停止を確認する。未対応NICや既存XDPがあれば止める。
3. [公開試験の票](PILOT_CHECKLIST.md)を埋め、公開経路・時間帯・rollbackを承認してから一件のcanaryへ進む。

各段階の対象・承認と前段の成功を確認した後に、その対象で起動します。以下は配置済みの標準unitの例で、隔離確認を飛ばして公開NICへ装着する指示ではありません。今回実行するものでもありません。

```sh
sudo systemctl start veil-local-ban.service
sudo systemctl status veil-local-ban.service
```

statusに加え[管理CLIと実通信](OPERATIONS.md)で確認します。unit起動だけで受入れを終えません。

`scripts/local-ban-operator-kernel-check.py`等はpi-nodeの固定path／UID／既存fixtureを要求する試験driverです。新host向けinstallerとして実行しません。適合時は検査を削らず、対象とhashを置き換えた案をレビューします。
試験終了時は[停止・非enabledの確認](RECOVERY.md)まで行い、継続運用・enable・host reboot・DNS／router変更を自動で追加しません。

## 検証記録

- [最新binaryの実XDP一覧・preview受入れ](../review/2026-10-25-LOCAL-BAN-OPERATOR-KERNEL-ACCEPTANCE.md)
- [以前の常設unit／専用userの隔離記録](../review/2026-10-25-LOCAL-BAN-FULL-RUN-ACCEPTANCE.md)
- [以前のhost bootとnamespace復元](../review/2026-10-25-LOCAL-BAN-BOOT-ACCEPTANCE.md)

以前のbinaryでのboot／復旧試験を、最新binary・物理NIC・公開siteの合格に読み替えません。
boot記録ではmainnet同一container IDの監査条件が未達、当時のCargo起動も失敗として保全されています。namespace復元／BAN0の成功とは区別し、全hostの無影響を示す証拠にしません。後の[build復旧記録](../review/2026-10-25-NIX-BUILD-RECOVERY.md)も別の証拠です。
