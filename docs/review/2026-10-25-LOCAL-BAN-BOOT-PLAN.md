# ホスト再起動・namespace復元・systemd自動起動の隔離試験計画

2026-10-25 JST。**以下の範囲への明示承認後、一度だけ正常再起動を実施。namespace復元・自動起動・BAN消失を確認し、停止・disabledへ戻した。** host監査の未達条件とビルド環境の制約を含め、[実施報告](2026-10-25-LOCAL-BAN-BOOT-ACCEPTANCE.md)を参照。

最終ゴールは、小規模公開サイトで安全に試用できる無償OSS。現在の節目はローカル一時BANの起動・復旧運用の確認。
今回はホストを一度だけ正常再起動し、namespace復元→systemd自動起動→BAN 0を、実際のbootを跨いで確認する。最後は停止・disabledにする。

## 現状と避けられない影響

- pi-nodeはSSH接続名。実ホスト名は`maakie-lab`、Linux x86_64、現在kernel 7.0.0-38-generic。
- `ns-veil-rogue`、host側`veth-victim` 10.99.0.1/24、namespace内`veth-rogue` 10.99.0.66/24は現在存在するが、ホスト再起動で消えるruntime構成。今回、同じ名前・MAC・IP・MTUの復元を追加する。ifindex、namespaceのinode、processのPIDは再起動で変わる。
- 既存の試験用2 unitはinactive/disabled。既存のprimary/recovery・破損fixture・アカウント・unitを今回の制御先に使い直さない。
- **ホスト再起動は隔離環境だけに閉じない。SSH接続とログインsessionが切れ、Docker、mainnet、WireGuard、NTP等も一時停止・再起動する。** 設定の変更はしないが、「他サービスを無停止で保全」は不可能。
- 現在`mainnet`コンテナが稼働中。restart policyはunless-stopped、StopTimeoutは120秒。既存の正常停止手順に従う再起動でも、既存timeoutを超える停止ではDocker等の通常動作による強制終了があり得る。今回その設定を緩めたり、独自にforce rebootしたりしない。
- 現時点で`/run/reboot-required`は未存在。既存の自動更新timer等を無効化しないため、起動時の通常処理は発生し得る。
- 既存データ保全は、試験外の設定や保存データを直接変更・削除せず、既存fixtureと設定hashを確認する意味。再起動に伴う通常のlog/DB書込み、停止前のRAM状態まで不変にする意味ではない。

## 承認対象となる変更・操作

| 対象 | 操作と範囲 |
| --- | --- |
| `/opt/veil-local-ban-pi-boot/` | root0755を新規作成。固定namespace helper `namespace.py` 0555、service.conf0600、導入manifest0600を新規配置 |
| `/var/lib/veil-local-ban-pi-boot/primary` | root0755の専用親と、既存専用UID979所有0700の新規空primary。過去fixtureを移動・修復・削除しない |
| `/etc/systemd/system/veil-local-ban-pi-namespace.service` | 新規0644のoneshot。固定namespaceの検査・復元だけ。成功時RemainAfterExit。stop時のnamespace削除なし |
| `/etc/systemd/system/veil-local-ban-pi-boot.service` | 新規0644。namespace unitをRequires/Afterにし、NetworkNamespacePathを固定。既存のroot所有binary/objectを読取り利用するが既存unit/config/stateは変更しない |
| `/etc/systemd/system/multi-user.target.wants/veil-local-ban-pi-boot.service` | **この1本だけをenableで新規作成し、試験後のdisableでこの所有確認済みsymlinkを除去することを含めて承認対象とする。** namespace unitはRequiresで起動し、直接enableしない |
| `/home/maakie/vlb-boot-<新規乱数>/` | 新規0700のstaging・証拠・試験用HTTPS証明書/key。証拠とkeyはprivateに保持し、本文を出力しない |
| runtimeネットワーク | 初回の事前確認は既存layoutが一致すれば変更なし。再起動後、全体が不存在の場合に限り`ip netns add`とveth新規作成、固定MAC/IPv4/MTU、UP設定を行う。`/run/netns`の標準的なbind mountを含む |
| ホスト | 事前確認成功後に`systemctl reboot`を**一度だけ**実行。force、再起動の自動再試行、bootloader変更なし |

アカウントは新設・変更しない。既存`veil-local-ban-pi-full`（UID/GID979、locked/nologin）を新unitのUser/Groupとして利用する。旧stateはReadWritePathsに含めない。
新unitに渡す制御先は新primaryだけ。daemonのcapability・資源上限・処理期限は既存profileを維持し、起動後CapEff/Prm/Inh/Amb=0を確認する。

配置物は[boot試験用unit/config](../../deploy/systemd/pi-boot-test/)、[固定namespace helper](../../scripts/local-ban-namespace.py)、[bundle hash](../../artifacts/boot-namespace-02j1usy3/bundle-sha256.json)を参照。
全配置先・enable先の不存在を再確認し、O_EXCLで新設する。未知の既存物に上書き、再帰chown、削除は行わない。

## namespace復元の制約

helperは固定名・固定subnetだけを扱い、引数で任意namespaceや物理NICを指定できない。
既存layoutを検査する場合は、MAC・相互peer index・IP・MTU・route・未知interface不在・process不在・XDP不在を確認する。
全体が不存在なら新設する。片方だけ残る構成、IP/route衝突、未知のmount等は保全して失敗する。delete/flush/replace、default route追加、forwarding、UFW、NetworkManager設定変更はしない。
途中失敗も自動修復や削除をせず止まる。

namespace作成にはCAP_SYS_ADMIN/CAP_NET_ADMINが必要なので、このoneshotだけrootで動く。bind mountをhost側へ見せる必要があり、filesystemによるprivate mount sandboxは付けない。root所有の固定scriptをPython isolated modeで実行し、RPCや外部入力は持たない。通常daemonにこれらの追加権限を渡す設計ではない。

## 実行順序と合格条件

1. 旧fixture・unit/config・アカウントのhash/metadata、boot ID、主要サービスの状態・enabled状態、mainnetのcontainer ID/running/restart policy、物理NICの設定を記録する。秘密の本文・環境変数は取得しない。
2. 新規配置後、実配置の`systemd-analyze verify`を行う。namespace unit→daemonの手動起動で依存関係・権限・通常HTTPS・対象外portを確認する。未知物や他processがあれば停止し、再起動しない。
3. 既存の変更間隔で256件を投入し、257件目の拒否・既存BAN維持を確認する。再起動判定用のTTLは有限3600秒（既存上限86400秒以内の試験入力。上限や期限計算の変更ではない）。実通信元のBANを含め、再起動直前に新instance前の状態と遮断を記録する。
4. 新boot unitだけをenableし、想定のsymlink1本だけが作成されたことを確認する。namespace unitは依存起動でありdisabledのままでよい。
5. 正常なホスト再起動を一度実行する。SSH復帰は最大5分観測し、戻らなければ再起動や設定変更を繰り返さず、連絡・console復旧へ切り替える。
6. 再起動後にstart命令を送る**前**に、boot ID変化、新unitのenabled/active、namespace helperの今回bootでのrestored記録、起動順序とnamespace所属を確認する。daemonの自動起動を手動startの成功で代用しない。
7. 新instance・capacity256・occupied0を確認。client側の経過時間が前のTTLより十分短いことも記録し、自然失効と混同しない。HTTPS、短期TTL失効・個別解除、旧instanceの要求拒否を確認し、Linux library回帰も行う。
8. 新boot unitを停止・disableし、許可したenable symlinkだけが除去されたことを確認。namespace unitも停止する。両unitはinactive/disabled、XDP非装着、試験listener/専用UID processなし。namespace/veth自体は削除せず残す。
9. 旧fixtureとunit/configの保全、主要サービスのactive/enabled、mainnet containerの同一性とrunning状態を前後比較する。PID一致は再起動後の合格条件にしない。アプリの論理的整合性・同期完了まで保証したとは扱わない。

製品Rust/BFT/WAL保存方式・sync_all・256件上限・期限仕様は変更しない。今回の新機能は隔離ネットワークのboot時復元と起動依存関係だけ。

## 不具合時の復旧・停止条件

- 再起動前の問題なら再起動せず、新unitだけ停止・disableする。起動用symlinkの内容/identityを確認し、未知のsymlinkなら削除せず止める。
- 再起動後にSSHが戻れば同様に新unitを停止・disableし、XDP非装着を確認する。未知のnamespace/部分構成を自動削除しない。
- SSHが戻らなければremoteでの復旧は保証できない。利用者のconsole等から次を実行する。**再起動自体や失われたRAM状態は取り消せない。**

```sh
sudo systemctl stop veil-local-ban-pi-boot.service veil-local-ban-pi-namespace.service
sudo systemctl disable veil-local-ban-pi-boot.service
```

- 必要なら今回新設したunitだけをhash/identity照合後、各`.service.disabled`へ上書きなしで退避しdaemon-reloadする。退避先が存在すれば停止する。
- 主要サービスが自然に復帰しない場合、成功とせず状態を報告する。その設定変更や独自の強制再起動は今回行わない。
- 成功時も新設unit/config/helper/state・証拠は削除せず残す。アカウントDB全体の復元、旧fixtureの修復、物理NIC・既存firewall・NTP・Docker/pi-mainnet設定変更は行わない。

最終的にdisabledへ戻すため、以後のホスト再起動でこの試験namespaceが再作成される常設運用にはならない。本番でのenableや入口選定は別判断。

## 承認前に確認できた範囲

helperのmocked検査12件成功。pi-nodeでのhelper **inspectだけ**もverified_existing/changed=false。新しい配置先とunit/enable先は未存在だった。
これはnamespace新設、実unit検証、自動起動、host再起動の成功を意味しない。これらは承認後の実機確認事項。
