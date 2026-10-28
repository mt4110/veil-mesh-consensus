# 専用ユーザー・常設systemd unitの隔離受入れ計画

2026-10-25 JST。承認前に作成した計画。**その後ユーザーの明示承認を受けて実施し、停止・disabledで残した。** [実施結果と残る課題](2026-10-25-LOCAL-BAN-PERMANENT-SYSTEMD-ACCEPTANCE.md)を参照する。
最終ゴールは小規模公開サイトで安全に試用できる無償OSS。現在の節目はローカル一時BANの運用受入れ。
今回は、前回代用したnobody/一時unitを専用アカウント/常設unitへ置き換え、起動・停止・再起動・手動復旧を検証する。

## 読取り専用の現状確認

- pi-node: systemd 259.9。`veil-local-ban`、`veil-local-ban-pi-test`はいずれも未登録。
- `veil-local-ban-pi-test.service`はLoadState=not-found。下表の新規配置先も存在しないことを確認した。
- 既存`ns-veil-rogue`内の`veth-rogue`はUP、XDP非装着。TCP 18443/18444には待受けがない。
- 既存namespaceやvethを作り直さない。試験時も既存XDP/他の待受けが見つかれば開始しない。
- `systemd-sysusers --dry-run`は専用user/groupに候補UID/GID 980を提示した。これは予約ではなく、実作成時の採番と衝突不在を再検査する。
- 製品binaryとobjectは前回の最終検証hashに一致した。新しいBFT/WAL改修や依存追加は行わない。

## 承認対象の永続変更

| 対象 | 変更と権限 |
| --- | --- |
| 専用user/group `veil-local-ban-pi-test` | systemd-sysusersで新規作成。nologin、homeなし、locked password、補助groupなし。既存アカウントは流用・変更しない |
| `/etc/passwd`, `/etc/group`, `/etc/shadow`, `/etc/gshadow` | 上記アカウント作成に必要な標準ツールの更新。必要な内部lock/backupを含む。shadow/gshadow本文を表示・収集しない |
| `/opt/veil-local-ban-pi-test/` | root所有の新規専用領域。binary 0555、object 0444、設定とsysusers定義を配置。サービスUIDには書換え不可 |
| `/var/lib/veil-local-ban-pi-test/` | root所有0755の親と、サービスUID所有0700のprimary/recovery。BANは保存しない。所有記録・破損fixture・未知fileのfixtureを保持 |
| `/etc/systemd/system/veil-local-ban-pi-test.service` | root所有0644の常設unitを新規配置。Type=notify、起動capabilityと起動後の破棄、256件上限、既存の時間制限を維持 |
| 同unitの`.service.d/10-isolation.conf` | NetworkNamespacePathを既存`/run/netns/ns-veil-rogue`へ固定。対象は`veth-rogue`の`10.99.0.66:18443/TCP`のみ |
| 同unitの`.service.d/90-recovery.conf` | 手動復旧試験時に新規配置。ExecStartとReadWritePathsを固定recovery先へ切替。元primaryを読取り専用で保全 |

配置予定の[unit](../../artifacts/permanent-systemd-c93fa_mq/bundle/veil-local-ban-pi-test.service)、[隔離drop-in](../../artifacts/permanent-systemd-c93fa_mq/bundle/10-isolation.conf)、[復旧drop-in](../../artifacts/permanent-systemd-c93fa_mq/bundle/90-recovery.conf)、[設定](../../artifacts/permanent-systemd-c93fa_mq/bundle/service.conf)、[アカウント定義](../../artifacts/permanent-systemd-c93fa_mq/bundle/sysusers.conf)を準備した。
[BUNDLE SHA-256](../../artifacts/permanent-systemd-c93fa_mq/bundle-sha256.json)で固定する。

実行前に全配置先と同名アカウントを再確認する。未知の既存path、symlink、unit/drop-in、user/groupがあれば上書き・chown・修復せず停止する。
新規作成したdirectoryだけをchownし、既存directoryへ再帰的な権限変更をしない。

## 実行する操作と確認

- このunitだけに`daemon-reload`、start、stop、restart、SIGKILLによる異常再起動試験、必要なreset-failedを行う。daemon-reloadはPID 1の設定再読込みであり、他サービスのrestartを実行しない。
- 常設fileからロードされること、Transient=no、停止後もFragmentPathが残り再startできることを確認する。
- 専用UID、補助groupなし、稼働中全threadのCapEff/Prm/Inh/Amb=0、NoNewPrivs=1、配布profileの資源上限を確認する。
- TLSで隔離入口へ通常通信し、BAN、期限失効、解除、再起動後の新instance/BAN 0を確認する。
- 通常の変更間隔で256件を投入し、257件目がCAPACITYとなり、既存のBANが追い出されないことを確認する。期待値・TTL・時間制限を緩めたり、成功まで命令を自動再送したりしない。
- 正常停止後に、この試験で新規作成したprimaryの`socket-owner`だけを意図的に破損させる。元bytesとmetadata/hashを証拠に保全し、未知fileのfixtureを加える。既存データへ障害注入しない。
- 起動拒否と不必要な再起動がないことを確認してから、同じ常設unitへ90-recovery.confを追加して固定の空recoveryへ切り替える。元primaryの全entryを保全し、書込み用openがEROFSになることを確認する。
- 最後にサービスを停止し、所有XDPの非装着を確認する。unitはenableせず、停止・disabledのまま残す。ホスト再起動は行わない。
- 前後の主要サービス状態、公開アカウント情報の既存entry、試験領域外への変更有無を確認する。自然に変動するhost全状態の完全不変を証明したとは扱わない。

## 不具合時の復旧と試験後の扱い

1. 対象を確認して`systemctl stop veil-local-ban-pi-test.service`。制御対象はこの試験用unitのみ。XDP非装着・隔離HTTPSの回復を確認する。
2. unit本体も保全して退避する必要があれば、作成済みunitの内容とidentityを照合して、`/etc/systemd/system/veil-local-ban-pi-test.service.disabled`へ**上書きなしで退避**し、daemon-reloadする。退避先が既にあれば止まる。未知のunitをrenameしない。drop-inは保持されるので、退避だけでLoadState=not-foundになったと仮定しない。退避後のstartは行わず、停止・XDP非装着を確認する。再稼働前にはunit本体とdrop-inを一体で確認する。
3. アカウントはlocked/nologinのまま保持し、専用UIDを削除・再利用しない。配置物、primary、recovery、drop-in、証拠も削除しない。
4. アカウントDB全体を古いbackupで戻さない。他ユーザーの後続変更を失うため。部分導入で止まった場合も、再実行で既存物を取り込まず、状態を報告して止める。

これは**稼働状態を停止へ戻す復旧**であり、全file/accountを削除して導入前と同じにする処理ではない。完全撤去は対象を改めて示して別途承認する。
成功時も試験用アカウント・常設unit・証拠は保持するため、ホストには上表の永続変更が残る。

既存BFT/WAL・鍵、他サービスのunit、UFW/NTP、物理NIC、他namespaceは変更対象外。
前回の一時unitの成功を今回の専用UID・常設unitの成功に置き換えず、実施後に結果を別途報告する。

## 現時点の判定

計画作成時点では事前確認・配置内容の準備までで承認待ちだった。**その後の承認に基づく常設隔離試験の機能確認は完了。** [実施報告](2026-10-25-LOCAL-BAN-PERMANENT-SYSTEMD-ACCEPTANCE.md)に失敗・続行・最終状態を区別した。さらに別途承認した新規対象で[修正版driverの単一通し受入れも完了した](2026-10-25-LOCAL-BAN-FULL-RUN-ACCEPTANCE.md)。既存fixtureは保全している。
検証driverは[専用script](../../scripts/local-ban-permanent-systemd-check.py)として用意し、preflight以外はrootと明示的な`--apply`を必要とする。これは実行ミス防止であり、人の承認を代替しない。計画準備時点では構文検査のみであった。実施後の結果・driver修正・段階続行の記録は上の実施報告へ分けて保存した。
前回の一時unit試験は[前回の運用報告](2026-10-24-LOCAL-BAN-SYSTEMD-OPERATIONS.md)を参照する。
