# 修正版scriptの単一通し受入れ結果

2026-10-25 JST。[新規対象と復旧方法の計画](2026-10-25-LOCAL-BAN-FULL-RUN-PLAN.md)へのユーザーの明示承認後、pi-nodeで実施した。

**新対象`veil-local-ban-pi-full`で、修正版scriptの通し試験は成功した。** 途中再開・試験の再実行・変更命令の自動再送はしていない。前回の分割実行記録は保全したまま、今回の結果を別に記録した。

最終ゴールは小規模公開サイトで安全に試用できる無償OSS。現在の節目であるローカル一時BANの専用UID・常設unitによる隔離運用受入れと、今回の単一通し確認は完了した。実サイトへの導入・自動起動・ホスト再起動の受入れは未実施。

上記はこの実行時点の記録。その後、別途承認した新boot試験用unitで[ホスト再起動・namespace復元・自動起動を確認した](2026-10-25-LOCAL-BAN-BOOT-ACCEPTANCE.md)。このfull-run対象と既存fixtureは停止・disabledで保全したまま。

## 実行と判定

| 項目 | 結果 |
| --- | --- |
| 新対象 | `veil-local-ban-pi-full`、UID/GID 979。locked/nologin、home `/nonexistent`、補助groupなし |
| 導入 | 新規不存在のpreflight後、installを一回実行。実配置のsystemd-analyze verify成功 |
| 受入れ | acceptを一回実行。native exit 0、38箇所の確認が成功。`complete=true`、`full_single_run_passed=true`、`cleanup_ok=true` |
| 続行なし | `--resume-result`不使用。新対象ではその指定自体を拒否。結果にcontinued_from/resume_stageなし |
| 最終状態 | inactive、disabled、MainPID=0、Transient=no、Result=success。隔離XDP非装着、試験listenerなし、専用UIDのprocessなし |

実行したscriptのSHA-256は`d7f1156420df516caec6b8bbdc4681465470c2ef3b3ec75abad5b8a93f46b1c0`で、承認前に固定したものと一致する。
保存方式・sync_all・TTL・時間制限・安全条件を変更していない。製品Rust code、object、Cargo関連を含む68ファイルは作業前後で一致し、pi-nodeのビルド元とも同じ68ファイルのhashが一致した。

## 確認した動作

- 通常HTTPSはIP SANとこの実行固有のbodyで照合。対象`10.99.0.66:18443/TCP`だけBANされ、別port18444は通信を継続した。
- TTL 3秒のkernel失効、分割掃除、明示的な個別解除が成功した。
- 256件を既存の変更間隔で投入し、257件目はCAPACITY。実通信元の既存BANが追い出されないことを確認した。
- 正常restartで256件のBANが消失。SIGKILL後の自動再起動でも新instance・occupied=0・HTTPS復帰を確認した。
- 正常停止後に同じ常設unitを再startできた。停止後は所有記録だけが残り、BANは保存されていない。
- 今回新設したprimaryだけに所有記録破損と未知fileを注入した。read-only診断は修復せず、daemonはexit 1で起動拒否し、不要な異常再起動を起こさなかった。
- 同じunitへ実際の90-recovery.confを配置し、固定の空recoveryへ切り替えた。元primaryのUID/mode/inode/hashを保全し、サービスのmount namespace/UIDからの書込み用openはEROFSだった。
- recoveryでも実際にBANしてからrestartし、新instance・BAN 0・HTTPS復帰を確認した。
- 初回・SIGKILL後・recoveryの稼働threadで、専用UID、CapEff/Prm/Inh/Amb=0、NoNewPrivs=1を確認した。CapBndには起動用3 capabilityの上限が残る。

前回止まったinactive unit回収の条件も今回実際に発生した。`reset-failed`の「not loaded」を、正しいFragmentPath・inactive・MainPID 0・Result success・NRestarts 0を再検査した場合だけ扱い、以降へ正常に進んだ。権限エラー等を無条件に無視する変更ではない。

## 回帰と独立した最終監査

- Linux library回帰を今回再実行: **24 passed / 0 failed / 1 ignored**。ignoredは従来の初期化SIGKILL用明示fixtureで、今回無効化したものではない。
- 準備時のdriver検査: 対象指定・不正指定拒否等9項目、error分類・既存file/symlink/directory保全等9項目を確認済み。実機の通し成功とは区別する。
- acceptの自己報告とは別に14項目を最終監査し、旧fixture一致、両unitの停止・disabled、両専用UIDのprocessなし、主要サービス、link状態等を確認した。
- 旧`veil-local-ban-pi-test`の17 entryは、mode/UID/GID/inode/device/size/typeと通常fileの内容hashが承認前baselineに一致した。旧UID/GIDは980のまま。
- passwd/groupの配置前entryは行hashが残っていた。shadow/gshadow本文は収集していない。
- SSH・Docker・timesyncd・UFWのLoadState/ActiveState/MainPIDは配置前と最終監査で一致した。host側と隔離interfaceのifindex/name/flags/XDPも一致した。全host file/processの完全不変を証明したという意味ではない。
- BFT全体の回帰は今回再実行していない。既存BFTのタイミング課題の解消を、このローカル方式の成功から推論しない。

## 残置物と制約

新unitも旧unitも停止・disabled。削除、enable、ホスト再起動、他サービスの再起動、物理NIC/UFW/NTP変更は行っていない。退避による復旧は不要だった。

承認どおり、新アカウントと`/opt/veil-local-ban-pi-full/`、`/var/lib/veil-local-ban-pi-full/`、常設unitと10-isolation.conf/90-recovery.conf、privateな証拠を残した。新primaryは意図的な破損fixture、recoveryは停止後の所有記録を保持する。unitの現在の管理先はrecoveryであり、primaryを修復して戻していない。

**今回残っていた「修正版scriptを一回で最後まで通せるか」の課題は解消した。** 本番導入判定には、実サイトの入口・保護IP・NIC・既存防御との関係を決めた受入れが必要。今回は既存namespace/vethを使ったIPv4/TCP試験であり、ホスト再起動後のnamespace復元・自動起動、IPv6、CDN越しの実client IP、攻撃検知の精度、回線帯域を埋める攻撃への耐性は保証していない。

## 証拠

- local: `artifacts/permanent-full-5gnhuj_1/`
- [accept結果](../../artifacts/permanent-full-5gnhuj_1/pi-accept.json)、[独立監査判定](../../artifacts/permanent-full-5gnhuj_1/final-checks.json)、[監査詳細](../../artifacts/permanent-full-5gnhuj_1/pi-final-audit.json)
- [Linux回帰log](../../artifacts/permanent-full-5gnhuj_1/pi-library-regression.log)、pi-unit-journal.log、pi-unit-verify.log、native-source-hashes.jsonも同じprivate領域に保管。
- remote: `/home/maakie/vlb-full-gpfs75cn/`。導入`permanent-install-xi4wy3b1/`、単一受入れ`permanent-accept-_olsmf0x/`。
- 試験用秘密鍵はprivateなremote証拠領域に保持し、本文を取得・出力・報告していない。
