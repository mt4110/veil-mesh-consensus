# owner限定probe：PID 1参照の置換とoffline回帰

2026-10-27 JST。本人承認は、PID 1の直接参照をroot所有の事前参照へ置き換える最小修正とoffline検証まで。**hp／Pi／cloudへの接続・配置・権限変更・隔離実行は行っていない。**

後続の別承認により、hp-ubuntuで[owner限定の隔離試験一回と終了後の保全照合](2026-10-27-NFT-OWNER-ISOLATED-ACCEPTANCE.md)を完了した。以下はその前段の修正・offline検証の記録として保持する。

最終ゴールは公開サイトで安全に使える手動一時BAN。現在のマイルストーンはnftables候補の停止・回収機能の評価。今回の完了条件は、最小権限を維持した参照方法への変更、異常系を含む回帰、実機への変更対象と復旧案の更新。

## 変更内容

[probe](../../scripts/nft-owner-probe.py)の事前確認だけを変更した。PID 1のnet／pid／user namespaceを直接statせず、固定の`/run/veil-nft-owner-probe-20261027a/host-reference.json`とself namespaceを照合する。参照ファイルは、将来の承認後、配置時にrootが採取して新規作成する。今回その生成・配置はしていない。

- runtimeはroot所有0700、参照はroot所有0400・通常ファイル・単一リンク、最大4096 bytes。
- 固定directoryのFDを開き、そのFDに相対の固定basenameで参照を開く。両方O_NOFOLLOW・O_CLOEXEC、参照はO_NONBLOCK。作成・truncate・chmod・chown・修復fallbackはない。[PythonのFD相対open](https://docs.python.org/3.14/library/os.html#os.open)・[O_NOFOLLOWの適用範囲](https://man7.org/linux/man-pages/man2/open.2.html)。
- 読取り前後のinode・mode・owner・link数・size・mtime／ctimeと読取りサイズを比較。欠損・入替え等を成功へ変換しない。開いたFDは失敗時も閉じる。
- version・boot ID・三つのnamespace識別値だけのschema。余分／欠落／重複field・不正型・boolでの整数代用・破損・別bootを拒否。
- 自分のnet namespaceがhostと異なること、pid／user namespaceが一致することを従来どおり要求。CAP_NET_ADMINのみ、NoNewPrivileges、loのみ、専用cgroup、既存table拒否を維持。

unit引数、最大60秒・停止猶予5秒・Restart=no、空table最大2個、socket close／SIGKILLの試験内容は変更していない。BAN・TTL・256件契約・既存製品は変更対象に含めない。

## 検証

```sh
python3 -I -B scripts/nft-owner-probe-test.py
python3 -I -B scripts/nft-owner-probe.py --offline-plan
```

[既存15件＋追加9件](../../scripts/nft-owner-probe-test.py)の計24件がMac上で成功した。実Netlink・実ip・fork・signal・systemd・sudoを起動する検証ではない。

| 追加した確認 | 方法と限界 |
| --- | --- |
| PID 1参照の回帰 | PID 1への参照をPermissionErrorとする同一の模擬OS条件で、修正前は失敗・修正後は成功。実機の最小権限processを起動した結果ではない |
| 参照とnamespace・権限 | 正常な参照でselfだけを確認。host net、別pid／user、余分なcapability、参照不在はip呼出しより前に拒否。kernel socket作成も禁止して検証 |
| 形式・同じboot | schema、version、UUID、別boot、識別値の型／数／範囲、重複・欠落・余分・破損・過大入力を拒否 |
| filesystem境界 | Macの新規fixtureで実open。symlink・hardlink複製・FIFO・directory・不在・過大サイズを拒否。root所有というfstat応答は模擬し、root権限・chownは使わない |
| 読取り障害・解放 | metadata変化・短い読取り・permission errorを注入。過大ファイルは内容を読まず拒否し、失敗でもFDを閉じる |

テストfixtureと旧sourceは新規private artifactsへ保全し、自動削除していない。所有者を模擬した試験を、Linuxでroot0400が実際に読めた証拠として扱わない。対象kernelのowner回収、systemd制限、実機のOS認証は引き続き未検証。

証拠は[今回のdirectory](../../artifacts/nft-owner-reference-z7d23rbc/)へ保存。[修正前の失敗](../../artifacts/nft-owner-reference-z7d23rbc/pid1-regression-before.log)と[同条件での修正後](../../artifacts/nft-owner-reference-z7d23rbc/pid1-regression-after.log)を別ファイルで保持した。先行した読取り失敗の原記録は[前回報告](2026-10-27-NFT-OWNER-READONLY-PREFLIGHT.md)に保全している。

[最終24件の出力](../../artifacts/nft-owner-reference-z7d23rbc/tests-final.log)と[集計・source hash](../../artifacts/nft-owner-reference-z7d23rbc/result.json)を保存した。従来のvalidate_boundary、unit引数、owner batch、Netfilter、case実行、全体実行の六つはAST比較でも変更なし。既存製品主要七ファイルは開始時hashと一致し、probeの文字列定数からPID 1 pathがなくなったことも確認。[plan出力](../../artifacts/nft-owner-reference-z7d23rbc/offline-plan.json)はlive_executed=false、ban_apply_supported=falseを維持する。[文書全体97ファイルのlint](../../artifacts/nft-owner-reference-z7d23rbc/markdown-lint.log)は指摘0。

## 実行前に確認する範囲と復旧

| 項目 | 将来の承認対象 |
| --- | --- |
| 新設物 | hpの`/run/veil-nft-owner-probe-20261027a/`、script、root0400のhost-reference.json、0700のevidence。既存なら上書き・削除せず中止 |
| 一時unit | `veil-nft-owner-probe-20261027a.service`、PrivateNetwork、loのみ、CAP_NET_ADMINのみ、最大60秒・停止猶予5秒・Restart=no |
| kernel変更 | 専用namespace内のowner空table最大2個とその回収。chain・rule・set・hook・BANなし |
| 影響 | 物理NIC・既存firewall・経路・他service・Pi・routerへ変更しない。共有kernelの資源・不具合の残余リスクはある |
| 復旧 | 所有を確認した今回unitだけ停止。専用process／socket終了、table不存在、SSHと事前状態を確認。host全体のflush・再起動・未知所有物削除なし |
| 終了 | 証拠を退避し、unit停止・非enabled。新設ファイルは保全。回収を観測できなければ成功にしない |

[全試験案・schema・打切り条件](../local-ban/NFT_OWNER_PROBE_PLAN.md)を更新した。今回の承認は上表の実行承認に読み替えず、隔離実行は保留する。

**残り:** この固定範囲への実行承認 → owner限定の隔離試験一回と保全照合。ownerが成功しても、nftablesのBAN適用・現行絶対期限との同等性・公開pilotは未達。期限条件を黙って変更しない。

モデル／推論: 継続指定Sol／High。現在sessionの実行記録はgpt-6.1-sol／high。分担なし。今回の入力・cache入力（入力の内数）・出力使用量は未取得。新規依存・追加課金試験なし。
