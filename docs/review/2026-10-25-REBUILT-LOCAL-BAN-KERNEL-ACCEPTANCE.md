# 再ビルドしたveil-local-banの実kernel隔離受入れ

2026-10-25 JST。**再ビルドしたbinaryそのものをpi-nodeの隔離環境で検証し、通信・256件上限・期限失効・解除・再起動時のBAN消失を確認した。**

最終ゴールは小規模公開サイトで安全に試用できる無償OSS。今回は、[Nixパス復旧後に新規コンパイルした成果物](2026-10-25-NIX-BUILD-RECOVERY.md)が、実際のkernel/XDPでも契約どおり動くことの確認。
今回の機能確認は完了。実サイトへの導入、物理NIC、IPv6やCDN越しの受入れを完了したという判定ではない。

## 使用した実体と試験方法

- 新binary: `/home/maakie/vnr-a4dsem9l/target/debug/veil-local-ban`
- SHA-256: `00dff5be88570c7506d05995cae79fac03614c18a24a6c16cf2431416881cc19`
- object: 既存のroot所有`/opt/veil-local-ban-pi-full/veil_xdp_kern.o`、SHA-256 `2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f`。
- 隔離先: 既存`ns-veil-rogue` / `veth-rogue`、対象`10.99.0.66:18443/TCP`。対象外port18444も同時に検査。

永続変更を避け、[専用driver](../../scripts/local-ban-rebuilt-check.py)から一時systemd unitを一回作成した。
そのunitのmount namespace内だけで、新binaryを既存の実行fileの位置へread-only bindした。**host側の旧binaryは交換・上書きしていない。**
初回・通常restart後・SIGKILL後のそれぞれで、実行中PIDの`/proc/PID/exe`を読み、SHA-256が新binaryと一致することを確認した。同時にhost側の旧binaryが従来の`f9cf2d…`であることも確認。command lineに見えるpathだけで新旧を判定していない。

既存専用UID/GID979を利用し、管理directory・driver・証拠・試験用証明書は新規の/run配下に限定した。アカウント作成、常設unit配置、enable、fstab変更、追加のhost rebootは行っていない。

## 確認した振る舞い

| 対象 | 実測・確認結果 |
| --- | --- |
| 正常通信 | daemon起動前後ともHTTPS成功。証明書のIP SANとrun固有bodyまで照合 |
| 実体・権限 | 全3世代のPIDで新binary hash一致、実際のnetwork namespaceも隔離先と一致。全threadがUID979、CapEff/Prm/Inh/Amb=0、NoNewPrivs=1 |
| 無期限BAN拒否 | TTL=0をINVALID_TTLで拒否 |
| 遮断範囲 | 実通信元をBANし、18443は遮断・kernel drop counter増加。18444は通信継続 |
| 期限失効 | TTL3秒のBAN後、daemonをSIGSTOPした状態でもkernel側で通信復帰。userspaceの掃除で解除された結果と混同していない |
| 分割掃除 | SIGCONT後、既存の4秒観測内でoccupied0。max_examined/max_attemptedは16以下 |
| 個別解除 | 新たなBANで遮断を確認後、unbanでHTTPS復帰 |
| 256件上限 | 実通信元を含む256件がoccupied/activeとも256。既存の変更間隔で投入し、257件目はCAPACITY。実通信元のBANを追い出さず、対象外portは通信継続 |
| 正常restart | 256件を保持したままsystemctl restart。新instance・occupied0・HTTPS復帰 |
| 旧要求の再送 | restart前のinstanceを使うBAN要求はINSTANCE_CONFLICT。BANを復活させない |
| SIGKILL | 新たなBANで遮断を確認後、mainだけをkill。既存Restart設定で自動再起動、新instance・occupied0・HTTPS復帰 |
| 最終状態 | XDP非装着、namespace内process/試験listener/UID979 processなし。今回の管理directoryに残るのは72-byteのsocket-ownerだけ |

27箇所の確認が一回の実行で成功し、complete=true/cleanup_ok=true。途中再開や成功までの変更命令再送は行っていない。
製品code、WAL保存方式、sync_all、TTL上限、256件上限、IO/起動期限、既存StartLimit/Restart条件を変更していない。今回のrestartはdaemonのrestartであり、host rebootを追加したわけではない。

## 保全と停止・非自動起動の確認

- 既存test/full fixtureとboot fixtureのmetadata/hashは前後一致。既存所有記録、破損fixture、未知fileを削除・修復していない。
- 既存binary・新binaryとも内容hashが一致。元binaryを配置し直して戻す処理も不要だった。
- 主要サービスのunit/drop-in hash・状態・PID、mainnet container情報、passwd/group entry、boot IDが一致。host interfaceのifindex/name/MAC/MTU/flags/XDPも一致。
- 既存4 unitはすべてinactive/disabled/MainPID0を維持。
- 今回の一時unit `veil-rebuilt-f3f00fa7f3e5.service` は停止後not-found/inactive/MainPID0で、自動起動symlinkもない。**一時unitにUnitFileState=disabledという値があるとは表現しない。** 常設unitを作らず、停止後に消滅して自動起動設定が残らない方式。
- driver自己報告とは別に、最終unit状態、XDP、namespace PID、listen port、UID979 process不在を再監査した。

systemd/journalの通常動作に伴うlogは生成されるが、hostの常設設定・binary・既存管理領域への永続変更は行っていない。
これは対象を絞った保全確認であり、host上の全fileやmainnetの論理的データ整合性を完全証明したという意味ではない。

## 証拠と残る範囲

- local: `artifacts/rebuilt-kernel-6kmgovmo/`
- [集約](../../artifacts/rebuilt-kernel-6kmgovmo/acceptance-summary.json)、[全確認結果と実行PID hash](../../artifacts/rebuilt-kernel-6kmgovmo/result.json)、[独立した最終監査](../../artifacts/rebuilt-kernel-6kmgovmo/independent-final-audit.json)。before.json/after.json、command.json、unit-journal.log、execution-source.jsonも保存。
- remoteの一時証拠: `/run/veil-rebuilt-evidence-fxn9wgy7/`。管理先は`/run/veil-rebuilt-runtime-esgzy_hw/control`。今回の新規領域は手動削除せず保持しており、次のhost rebootでは失われるtmpfsデータ。必要な非秘密の証拠はlocalへ取得済み。
- 試験用秘密keyの本文は取得・表示していない。

今回で、復旧手順による再ビルド→Cargo library回帰→その新binaryの実kernel確認まで接続できた。今回library回帰を再実行したとは扱わず、[直前のCargo回帰結果](2026-10-25-NIX-BUILD-RECOVERY.md)とこの実機確認を別の記録にする。
次の実サイト受入れには入口・NIC・保護IP・既存防御の選定が必要。今回のveth上のIPv4/TCP試験から、物理NIC全種・IPv6・CDN・攻撃検知精度・回線飽和への耐性まで保証しない。
