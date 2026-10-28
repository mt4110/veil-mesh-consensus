# 修正版scriptの新規対象による通し試験計画

2026-10-25 JST。**ユーザーの明示承認後に実施し、途中再開なしの通し試験に成功した。新unitは停止・disabled。** [実施結果](2026-10-25-LOCAL-BAN-FULL-RUN-ACCEPTANCE.md)を参照。

最終ゴールは、小規模公開サイトで安全に試用できる無償OSS。現在の節目は、ローカル一時BANを専用ユーザーと常設systemd unitで運用できることの確認。
今回は修正版scriptを新規対象で最初から最後まで実行し、途中再開なしの一つの受入れ結果を得る。

## 既存物と新規対象

既存の`veil-local-ban-pi-test`アカウント、unit、`/opt/veil-local-ban-pi-test/`、`/var/lib/veil-local-ban-pi-test/`、fixture、過去の証拠は変更しない。
新規対象は固定名`veil-local-ban-pi-full`。事前の読取り確認では同名user/group、下記配置先、unitは未存在だった。実行直前にも確認し、既存物やsymlinkがあれば上書き・修復せず中止する。

承認を求める永続変更は以下に限る。

| 対象 | 内容 |
| --- | --- |
| user/group `veil-local-ban-pi-full` | `systemd-sysusers`で新規作成。locked/nologin、home `/nonexistent`、補助groupなし。UID/GIDは未採番で、作成時に検査 |
| `/etc/passwd`・`group`・`shadow`・`gshadow` | 上記作成に必要な標準ツールによる更新と内部lock/backup。秘密の本文は収集・表示しない |
| `/opt/veil-local-ban-pi-full/` | root所有0755。binary0555、object0444、service.conf0600、sysusers.conf0444、90-recovery.conf0444、installation.json0600を新規作成 |
| `/var/lib/veil-local-ban-pi-full/` | root所有0755の親。専用UID所有0700の空primary/recoveryを作成。今回作るprimary所有記録にだけ破損を注入し、未知file fixtureも残す |
| `/etc/systemd/system/veil-local-ban-pi-full.service` | root0644、常設unitを新規配置。enableしない |
| 同unitの`.service.d/` | root0755。10-isolation.conf0644を新規配置。手動復旧の段階で90-recovery.conf0644を新規配置 |
| `/home/maakie/vlb-full-<新規乱数>/` | 専用0700の新規staging/証拠領域。script、bundle、HTTPS試験用の鍵・証明書と結果を保全。既存directoryは使わない |

配置内容は[新bundle](../../artifacts/permanent-full-5gnhuj_1/bundle/)、[bundle SHA-256](../../artifacts/permanent-full-5gnhuj_1/bundle-sha256.json)、[script SHA-256](../../artifacts/permanent-full-5gnhuj_1/driver-sha256.json)に固定した。
scriptの変更は、旧・新の2つだけを許す`--target`指定と結果の対象名・単一実行判定。新対象では`--resume-result`を拒否する。既存の修正（新規fileの正確なmode、systemdのinactive unit回収、復旧確認の変数初期化）は維持する。

## 実施と完了条件

1. 新対象の不存在と隔離入口の空きを再検査してから、installを一度実行する。
2. `accept --target veil-local-ban-pi-full`を、**途中再開なしで一回**実行する。既存のns-veil-rogue/veth-rogueだけで試験し、10.99.0.66:18443/TCPを遮断対象、18444/TCPを非対象の確認に使う。
3. 通常HTTPS、BAN、期限失効、個別解除、256件満杯、257件目拒否と既存BAN保全、通常再起動とSIGKILL後の新instance/BAN 0を確認する。
4. 今回新設したprimaryの所有記録を意図的に破損し、未知fileを加える。起動拒否とfixture保全を確認してから、同じunitを固定の空recoveryへ切り替える。元primaryの読取り専用化、復旧後BANと再起動による消失を確認する。
5. 最後に新unitを停止する。inactive、disabled、MainPID=0、所有XDP非装着、試験listener/専用UIDのプロセスなしを独立して確認する。旧fixture/unit/account、主要サービスとホスト側linkも前後比較する。

製品binary/object、256件上限、再起動でBANが消える契約、TTL・時間制限・capability条件を変更しない。BFT/WAL保存方式・sync_allも変更対象外。
プロトコル変更命令の自動再送や成功までの試験再実行は行わない。失敗した場合は、その一回を失敗として保存し、途中再開を通し合格と扱わない。

## 不具合時の復旧と残置

新unitの内容・配置identityを確認してから`systemctl stop veil-local-ban-pi-full.service`を実行し、XDP非装着と隔離通信の回復を確認する。
必要なら、この試験で新設したunitだけを、同じ場所の`veil-local-ban-pi-full.service.disabled`へ上書きなしで退避し、daemon-reloadする。既存の退避先があれば停止する。drop-inは残るため、退避だけでnot-foundになると仮定せず、停止状態を確認する。

成功時も停止・disabledのまま、アカウント・配置物・破損fixture・証拠を削除せず残す。userを削除/再利用しない。account DB全体をbackupで戻さない。完全撤去は別途の対象提示と承認が必要。
daemon-reload、start/stop/restart/SIGKILL/reset-failedは新unitを対象に行う。既存の試験unitや他サービスを再起動しない。ホスト再起動、enable、UFW/NTP/物理NIC/他namespace変更は行わない。

これは隔離環境での運用受入れであり、実サイトへの導入許可や本番保護の証明ではない。
