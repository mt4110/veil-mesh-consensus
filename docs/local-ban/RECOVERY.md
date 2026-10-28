# 停止・所有記録の診断・手動復旧

[README](../../README.md) · [導入](INSTALL.md) · [操作](OPERATIONS.md) · [試験前チェック](PILOT_CHECKLIST.md)

復旧は、管理者が承認したhost・unit・NIC・control pathだけを対象にします。未知のsocket・file・XDP・他processを削除／停止して通す手順ではありません。
一時BANは再起動で消えます。旧WAL・queue・所有記録からのBAN復元は行いません。基礎防御は独立して維持します。

## 1. 通信を戻す

| 状況 | 操作と確認 |
| --- | --- |
| 特定の有効BANだけ解除したい | [preview-unban→明示解除](OPERATIONS.md)。対象通信の復帰を確認 |
| 追加遮断をまとめて止めたい | [bypass on](OPERATIONS.md)。必要な現instance／control_revisionを確認できる場合だけ。通信で確認 |
| socket不応答、状態不明、tokenを取得できない | 所有確認済みのunitを停止し、XDP・通信を独立確認 |

標準unitの停止例:

```sh
sudo systemctl stop veil-local-ban.service
```

承認したunitと実NICに対し、停止後にstatus、XDP、process、対象HTTPSと管理経路を確認します。NICは実際の名前へ置き換えます。

```sh
systemctl show veil-local-ban.service -p ActiveState -p MainPID -p UnitFileState
ip -details link show dev REPLACE_INTERFACE
```

設定上は10秒のstop待ち後にSIGKILLしますが、OS／kernel待ちまで含めた即時復帰を無条件には保証しません。止まらない／XDPが残る／通信が戻らない場合は所有者とnamespaceを調べ、別のXDPを強制detachしません。
host reboot、firewall全flush、未知PIDのkill、`ip link ... xdp off`を自動実行しません。SSHを失った場合はconsoleから同じ対象を確認します。

## 2. 起動拒否の状態を変更せず診断する

```sh
sudo -u veil-local-ban /usr/libexec/veil-local-ban inspect-runtime \
  --control-dir /var/lib/veil-local-ban/primary
```

`INSPECTED`／exit 0は診断の実行成功です。正常・回収可能・通信復旧済みの意味ではありません。診断はsocket接続、bind、unlink、rename、記録修復をしません。

| 表示 | 判断 |
| --- | --- |
| owner_lock=busy | 別の所有者・起動処理があり得る。切替せず所有者を調べる |
| owner_lock=available | lockを取得できただけ。socketの死や所有の証明ではない |
| owner_record=valid | format／checksumの整合だけ。所有link・socket・生存確認も必要 |
| owner_record=missing／invalid、pending_record=present | 証拠を保全して止める。記録の新規作成・修復で迂回しない |
| socket_liveness=not_probed | inspectではsocketの生存を調べていない |

稼働中・初期化中のsnapshotは変わり得ます。namespace運用ならprocess／socket／XDPも同じnamespaceで調べ、host側に見えないだけで不在としません。
SIGKILL後でも、正しい記録・所有link・socketの一致と非生存を証明できる場合だけ通常起動が回収します。壊れた記録・未完成の初期化は保全して拒否します。

## 3. 同じunitを固定recoveryへ切り替える

変更前に対象・影響・戻し方をレビューし、承認します。標準配置での順序です。

1. `veil-local-ban.service`を止め、process／XDP／所有者を確認する。停止を把握できない、未知のXDPやlive ownerがいる場合はここで止める。
2. `/var/lib/veil-local-ban/primary`をその場に保全。socket・所有link・socket-owner・pending fileを削除／rename／chown／checksum修復しない。
3. 固定の`/var/lib/veil-local-ban/recovery`を新規0700・同じservice UID所有で作る。root所有の親の下へ作る場合も新規childだけを対象にする。既存なら止まり、掃除や`recovery-2`等で迂回しない。
4. 同じUIDでrecoveryをinspectし、未知entryがなく空であることを確認する。元primaryの診断をrecoveryの診断と取り違えない。
5. [復旧drop-in例](../../deploy/systemd/veil-local-ban.service.d/recovery.conf.example)を、同じunitの承認した新規drop-inへ配置する。`ExecStart=`と`ReadWritePaths=`を両方resetし、control-dirだけをrecoveryへ変更。binary／object／scope／保護IPは維持する。
6. daemon-reload、実効unitの確認、必要なら原因解消後にそのunitだけreset-failedしてからstartする。reset-failedは記録を直さず、起動rate制限もresetするので繰り返さない。
7. 新instance・capacity256・一覧0件／occupied0、同じscope、実HTTPSと管理通信を確認する。全管理clientのpathもrecoveryへ変更する。

元primaryへの書込みはunitのReadOnlyPathsで防ぎ、write可能なのはrecoveryだけにします。別のdaemonを同じNICへ並行起動しません。
recoveryも使えないなら停止し、原因と次の変更を別途判断します。元primaryへ自動で戻さず、証拠を削除しません。
以前のBAN commandは旧instanceとして扱います。再BANが必要なら、対象・理由・有限TTLを新しく人が判断します。

詳細と以前のbinaryでの異常系検証は[所有記録・手動復旧の記録](../review/2026-10-24-LOCAL-BAN-MANUAL-RECOVERY.md)、[systemdとの整合](../review/2026-10-24-LOCAL-BAN-SYSTEMD-OPERATIONS.md)、[常設unitの通し確認](../review/2026-10-25-LOCAL-BAN-FULL-RUN-ACCEPTANCE.md)を参照してください。

## 4. 試験を停止・非enabledで終える

常設unitは停止後に`systemctl is-enabled veil-local-ban.service`で確認します。初回はenableしません。誤ってenabledなら、所有する有効化symlinkの対象を照合し、その除去を承認した範囲で対象unitだけdisableします。disableだけでは稼働中serviceは止まらないため、停止と別に確認します。

| 配置方式 | 終了時の確認 |
| --- | --- |
| 常設unit | inactive／MainPID0、disabled、自動起動への参照が残っていない |
| 試験用の一時unit | inactive／MainPID0またはunload後not-found、enableしていない／自動起動symlinkなし。disabledという値があるとは表現しない |

さらに、所有XDPが外れ、試験process／待受が残らず、対象HTTPS・独立正常client・管理通信が復帰したことを確認します。unitの終了だけでは完了にしません。
設定のrollbackは承認した新設file／差分だけを戻します。未知のfileを削除・上書きしません。必要な退避は内容照合のうえ既存退避先を上書きしない方法を承認します。
account、配置binary、証拠、破損fixtureは削除せず保持し、調査・廃棄は別の判断にします。
