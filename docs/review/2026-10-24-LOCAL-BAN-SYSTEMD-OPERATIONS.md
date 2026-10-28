# ローカル一時BAN: systemdと最小権限の運用

2026-10-24。[初期化中断・手動復旧](2026-10-24-LOCAL-BAN-MANUAL-RECOVERY.md)に続く運用仕様。
2026-10-25追記: 専用ユーザーと常設unitでの隔離確認は[別の受入れ報告](2026-10-25-LOCAL-BAN-PERMANENT-SYSTEMD-ACCEPTANCE.md)へ記録した。以下の検証数・一時unitの説明は10-24の実施範囲を表す。
最終ゴールは、普通のブラウザが使う小規模公開サイトで、安全に試用・停止・復旧できる無償OSS。
今回のゴールは、既存BFTとデータを保全し、systemdによる起動・停止・再起動と権限制限を隔離Linuxで確認すること。

## 提供物と契約

- [unit](../../deploy/systemd/veil-local-ban.service)、[設定例](../../deploy/systemd/veil-local-ban.service.conf.example)、[専用アカウント定義](../../deploy/sysusers.d/veil-local-ban.conf)、[固定復旧先のdrop-in例](../../deploy/systemd/veil-local-ban.service.d/recovery.conf.example)。配置・enableは導入先で別途承認する。
- 実行UIDは専用の`veil-local-ban`。root daemon、ログイン利用者、共有の`nobody`を本番の実行主体にしない。管理clientと診断も同じUIDで実行する。
- BANはメモリとFD所有のBPF Mapだけ。正常停止、SIGKILL、自動再起動、手動再起動、ホスト再起動で消える。旧WALやsocket-ownerからBANを復元しない。
- 256件上限、満杯時の新規IP拒否、有限TTL、16 slot/200msの分割掃除、CAS/instance照合、既存XDPの置換禁止を維持する。
- `Type=notify`は、socket初期化・XDP装着・起動権限破棄・終了signal受付準備後にREADYを通知する。activeは実通信確認の代用ではない。
- 正常停止はSIGTERM。`KillMode=control-group`で子processも管理し、停止上限10秒の後はSIGKILL。通常のBAN契約の期限や管理RPCの制限は変更しない。
- `Restart=on-abnormal`、2秒待ち、同一ロード期間中の起動rate-limitは60秒に3回。inactive unitのGCやreset-failedを跨ぐ永続的な回数制限ではない。SIGKILL等は空のBAN状態で再起動する。所有不明・証明破損等のexit 1は自動再試行せず止まる。`systemctl stop`から勝手に再起動しない。

## 起動時だけ3 capability、その後はゼロ

既存objectのロード・XDP装着に`CAP_BPF CAP_NET_ADMIN CAP_PERFMON`を与える。pi-nodeではこの3つから1つずつ除いた負の試験で起動拒否を確認する。
特に`CAP_PERFMON`を除くと既存bytecodeのpointer演算がverifierに拒否された。`CAP_SYS_ADMIN`やrootへのfallbackは設けない。
ロード後はambientを消し、effective/permitted/inheritableをcapsetでゼロ化し、capgetで検査する。`NoNewPrivileges`も設定する。

Linux capabilityはthread単位なので、製品binaryはTokioの`current_thread`で開始し、`/proc/self/task`が1件の時だけ権限を落とす。複数threadでの不完全な破棄は拒否する。
その後に作るblocking workerは破棄後の資格情報を継承する。Mapの更新・削除・counter参照は既に開いたFDで行い、失敗すれば従来どおりエラーにする。

単一thread化後、従来の実機容量試験は64件目と158件目付近で`BUSY`を返して停止した。
掃除が状態lockを先取りし、blocking poolへの投入から実行開始まで保持することを確認した。
この待機区間は不要なので、**掃除のtry_lockをworker実行開始後へ移した**。ただし、これだけでは実行中の掃除との衝突が残り、容量試験が停止した。
最終形はBAN/解除の受付枠を1件とし、その1件だけがworker内で掃除のlock解放を待つ。競合する2件目は待たずBUSY。掃除とBANの排他、16件/200msの上限、Skip、200msの変更間隔は維持する。
唯一のblocking workerを占有する回帰を追加し、旧配置でlockを取れず失敗、新配置で取れて成功することを確認した。
待機も既存のjob完了待ち2秒に含め、時間制限を増やさない。jobが時間切れになっても受付枠と通常接続permitはworkerが保持し、処理が終わるまで次の変更を入れない。
この1件が掃除待ち/処理中/結果不明の間、競合する変更はBUSYになる。BUSY、応答期限切れ、適用結果不明を同じ意味に扱わず、結果不明の命令を自動再送しない。
追加回帰で「掃除待ちの1件は後で適用される、2件目はBUSY」「応答が2秒で時間切れになっても1件枠を保つ」を確認した。

`CapBnd`には起動用3 capabilityの上限が残る。**bounding setもゼロにしたという意味ではない。** 稼働後の全threadのEff/Prm/Inh/Ambゼロを実測する。

unitではfilesystemを基本read-onlyにし、専用の管理親directoryを`ReadOnlyPaths`で明示的に保護して、選択した子directoryだけを`ReadWritePaths`でwrite可能にする。privateな一時領域等のsystemdの例外はある。home、device、kernel設定、namespace生成、SUID/SGID、実行時権限獲得を制限する。
AF_UNIX/AF_NETLINK、system-service syscall群と明示的なbpf/prctlだけを許し、TasksMax=32、MemoryMax=128M、LimitMEMLOCK=8Mを置く。
BPF操作に必要なsyscallまで禁止してテストだけ通す設計にはしない。全kernel versionで同じprofileが通るとは主張しない。

専用UIDが侵害されれば、保持しているMap FDと管理権限の範囲でBANを操作され得る。capability破棄は、このdaemonそのものの侵害耐性や256件の強制的なkernel隔離を証明するものではない。

## 配置前の準備

今回は配布fileを作り、**一時unitのみを隔離環境で実行**した。`/etc`の書換え、ユーザー作成、常設unitのinstall/enable、物理NICへの装着は行っていない。
導入時は対象interface、IPv4/TCP宛先/port、保護IPを決め、基礎防御と独立した管理経路を維持する。

1. 同名アカウントがなければ、承認後にsysusers定義で専用アカウントを用意する。同名の既存ユーザーを取り込まず、UID・home・shell・補助group・既存processを確認する。不要なgroupは持たせない。
2. 承認済みbinaryを`/usr/libexec/veil-local-ban`、objectを`/usr/lib/veil-local-ban/veil_xdp_kern.o`へ配置する。root所有でサービスUIDには書換え不可、binaryにfile capabilityやsetuidを付けない。設定もroot所有でサービスUIDには書換え不可にする。
3. root所有0755の`/var/lib/veil-local-ban`と、サービスUID所有0700の`primary`を新規作成する。既存なら自動的にchown/chmod/上書きせず、所有・用途・内容を確認して止まる。
4. 設定例の文書用アドレスを承認済みの値へ置き換え、unit構文と実行pathを確認してからinstallする。

新規directoryだけを作る例。既存なら失敗させるため`-p`を付けない。**既存directoryへこの操作を繰り返さない。**

```sh
sudo mkdir -m 0755 -- /var/lib/veil-local-ban
```

root所有0755の親へサービスUIDは作成できない。rootが新規childを作成し、その**新規childだけ**をサービスUIDへ渡す。
例えば既存directoryを対象にしない次の一回限りの処理を使う。

```sh
sudo python3 - <<'PROVISION'
import os, pwd
account = pwd.getpwnam('veil-local-ban')
path = '/var/lib/veil-local-ban/primary'
os.mkdir(path, 0o700)  # EEXISTなら停止。既存fileのownerを変えない。
os.chown(path, account.pw_uid, account.pw_gid)
PROVISION
```

親directoryの確認と新規作成は別の導入操作として承認する。既存childを修復するためのscriptではない。
`RuntimeDirectory=`/`StateDirectory=`/tmpfiles掃除を使わず、systemdの自動削除・再帰chownで未知の証拠を変更しない。
`/var/lib`にはsocketの所有記録だけが残り、BANは保存しない。固定pathがホスト再起動でも残るため、毎回`/run`のdirectoryを再provisionする必要がない。
OSや管理者による削除、電源断、disk故障までこの契約で防ぐことはできない。

## 通常の操作

install後に承認した操作として実行する例。daemonへ直接SIGKILLを使う必要はない。

```sh
sudo systemctl start veil-local-ban.service
sudo systemctl status veil-local-ban.service
sudo -u veil-local-ban /usr/libexec/veil-local-ban status --control-dir /var/lib/veil-local-ban/primary
sudo systemctl stop veil-local-ban.service
sudo systemctl restart veil-local-ban.service
```

stop後にXDP非装着と対象HTTPS通信を確認する。restart後は新しいinstance、capacity=256、occupied=0を確認し、実際の入口でHTTPSを確認する。
管理clientはrootではなくサービスUIDで実行する。socketのUID認証はrootからの接続も同じUIDでない限り拒否する。
権限を得た管理者が`sudo -u`できることはローカル管理権限の一部であり、新たな遠隔APIやBANの自動承認を追加しない。

## 所有不明・破損時の手動復旧

既存の[手動復旧手順](2026-10-24-LOCAL-BAN-MANUAL-RECOVERY.md)の停止条件、未知のsocketの保全、別XDPの強制detach禁止を引き継ぐ。systemd運用では次のように具体化する。

1. `systemctl stop veil-local-ban.service`で**同じ1つのunit**を止める。停止を確認できなければ先へ進まない。
2. `sudo -u veil-local-ban /usr/libexec/veil-local-ban inspect-runtime --control-dir /var/lib/veil-local-ban/primary`で診断する。`INSPECTED`は回復成功の意味ではない。対象namespaceのXDPとsocket/process所有者も確認する。
3. 破損したprimaryをその場に残す。削除、rename、checksum再計算、chownで通さない。
4. 固定1か所の`/var/lib/veil-local-ban/recovery`を、上と同じ新規作成限定の方法でサービスUID所有0700にする。既存なら止まる。別名を増やして再試行しない。
5. [drop-in例](../../deploy/systemd/veil-local-ban.service.d/recovery.conf.example)を同じ`veil-local-ban.service`へ適用する。ExecStartとReadWritePathsを両方resetし、write可能なのはrecoveryだけにする。別のdaemonを同じNICへ並行起動しない。
6. `systemctl daemon-reload`後、原因が解消されたことを確認した上で必要ならこのunitだけ`systemctl reset-failed`し、`systemctl start`する。reset-failedはデータや証明を修復せず、起動制限の履歴もresetするため、原因未確認で繰り返さない。
7. recoveryの新instance・BAN 0と実HTTPSを確認し、管理client/監視のcontrol pathもrecoveryへ切り替える。以後のrestartも同じpathを使う。古い命令を自動再投入しない。

`ProtectSystem=strict`だけに依存しない。pi-nodeの当初の`/run`試験ではruntime mountがrwであり、元primaryへの書込み用openが成功した。専用親の明示的な`ReadOnlyPaths`を追加し、実配布と同じ`/var/lib`上の新規領域で、サービスUIDからの書込み用openがEROFSになることを検証した。
元primaryはdaemonからread-onlyになり、systemdも停止時に消さない。元pathへの復帰や証拠の廃棄は別の判断とする。

## 検証記録

| 検証 | 結果と範囲 |
| --- | --- |
| Linux library | 24 passed / 1 ignored。ignoredは初期化SIGKILL用の明示subprocess fixture |
| macOS library | 7 passed。portable engineのみ |
| 既存BFT・2並列 | 再実行は183 passed / 7 ignored / 3 filtered。初回は182 passed / QUIC応答期限切れ1 failed |
| systemd + 実XDP/HTTPS | 最終39項目が通過。製品binary、非root、全threadのcapability破棄、通常/異常restart、容量、保全/復旧を検証 |
| 従来の実機回帰 | 最終78項目が通過。既存scriptの期待値・制限を変えず実行 |
| 初期化中断・手動復旧 | 最終85項目が通過。初期化SIGKILL fixtureと製品binaryを区別して検証 |
| unit/drop-in構文 | pi-nodeのsystemd-analyze verifyで両方exit 0 |

初回のBFT回帰で`authenticated_quic_rejects_legacy_and_zero_bans_but_accepts_finite_and_unban`が`QUIC response deadline elapsed`で失敗した。単独再実行は通過し、2並列全体の再実行も通過した。
この失敗記録も保存する。1回の再実行成功からBFTのタイミング問題が解消したとは判断しない。BFT/WALのコード・条件は変更していない。

systemdの正の試験は配布unitのService設定を読み、実行UID、binary/objectの配置path、control親path、隔離network namespaceだけを試験用に置き換える。
専用アカウントを永続追加しないため、事前にprocess不在・補助groupなしを確認した既存`nobody` UID 65534をstand-inに使った。
構文検査もこのstand-inを使うため「nobodyは安全な本番ユーザーではない」という警告が出る。**配布unitは専用`veil-local-ban`を指定し、この代用を本番推奨にしない。**

systemd管理の`/etc`や常設drop-inを変更せず、一時unitを停止後に同じunit名・固定recovery pathで再定義した。実配布drop-inは構文検査までであり、常設unitへのインストール試験とは区別する。
一時unitは停止後にsystemdがunloadするため、単純な`systemctl start`のunit-not-foundを製品の起動拒否と誤認しないよう試験を修正した。
破損拒否の成立は製品processのexit 1、Result=exit-code、NRestarts=0で判定した。

- 起動時: nonroot、CapEff/Prm/Inh/Amb=0、NoNewPrivs=1を全threadで検査。異常restart後も同じ検査を実施。
- 256件を通常の変更間隔で投入し、257件目はCAPACITY。既存entryを追い出さず、満杯でも個別解除可能。命令の自動再送はしない。
- namespace内の対象HTTPSでBANによる遮断・kernel TTLによる復帰・解除・restart後の復帰を確認。IP SANとrun固有bodyを照合。
- 正常stopでXDP非装着・socket撤去、72-byte所有記録の保全を確認。SIGKILLでは同じ管理領域を自動回収して新instance/BAN 0で復帰。
- 破損primaryと未知fileをhash比較で保全。同じunit名で固定recoveryへ切替後も保全し、サービスのmount namespace内・サービスUIDから元の所有記録を書込み用にopenするとEROFSになることを確認。
- 起動capabilityを1つずつ取り除く3つの負の試験はいずれもexit 1・XDP非装着。root/SYS_ADMINへfallbackしない。

初期化SIGKILLのlibrary fixtureはtest harnessが複数threadを持つため`#[cfg(test)]`で起動権限破棄を実行しない。製品の権限破棄をこのfixtureの成功から推定せず、上記の実製品systemd試験で検査した。
試験processと所有XDP linkの終了を確認し、破損領域・未知file・新しい管理領域・ログは証拠として残す。UFW/NTP/物理NIC/既存WAL・鍵は変更していない。

一次証拠:

- local: `artifacts/systemd-local-wo7csv6r/`
- pi-node作業領域: `/home/maakie/vlbsd-dohBzYcq/`
- systemd最終結果: `systemd-5p19p_2k/result.json`（39項目）
- 従来実機回帰: `local-ban-9o45gtw1/result.json`（78項目）
- 初期化/復旧: `init-recovery-789kikjn/result.json`（85項目）
- unit構文検査: `unit-verify-rd2picxj/`

製品binary SHA-256: `f9cf2dc594cc0808b91247a8f0d447edcb7b9370ae8273e0f777deebe85da05a`。
旧BFTの保存方式・sync_all・署名/WAL関連ソース、object、Cargo.lockは開始時hashとの比較で保全を確認した。既存WAL/鍵データを移行・削除・書換える操作は行っていない。

## 判断と限界

**今回のsystemd運用変更は採用可。** 実製品binaryのsystemd配下で起動/停止/restart、異常再起動、所有破損拒否、別の固定管理入口への復旧、256件上限、全threadの権限破棄が成立した。
常設unitの実install/enable、ホスト再起動、別kernel/NIC、IPv6、CDN背後の実client IP、攻撃検知の精度、実サイトでの試験導入は別の確認である。
追加BANが消える契約なので、daemon停止中や復旧中にも維持すべき基礎防御の代用にはしない。

一次仕様: [systemd.service](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html)、[systemd.exec](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html)、[Linux capabilities](https://man7.org/linux/man-pages/man7/capabilities.7.html)。
Webのlatest仕様だけを導入先の互換性の根拠にせず、今回のpi-node systemd 259.9で実際のprofileを検証する。
