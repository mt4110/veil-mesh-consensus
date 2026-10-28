# nftables owner限定確認の準備記録

日付: 2026-10-27 JST。**実機実行なし。準備・オフライン検証だけ。**

## 判断と成果物

本人は「現行期限を維持する。実適用は保留し、owner機能だけの隔離確認案を準備する」を選択した。CLOCK_BOOTTIMEの絶対期限を変えず、nftablesへのBAN適用はBLOCKEDを維持する。

最終ゴールは小規模公開サイトで安全に使える手動一時BAN。マイルストーンはnftables候補の権限・停止境界の評価。今回の完了範囲は、空のowner tableだけを対象にした隔離案・script・復旧方法と、そのoffline検証までとした。

- [実機確認案](../local-ban/NFT_OWNER_PROBE_PLAN.md): hp-ubuntuの新規PrivateNetwork unit、loのみ、空table最大2個、chain／rule／setなし、60秒・停止猶予5秒、既存環境保全と中止条件。
- [限定probe](../../scripts/nft-owner-probe.py): Python標準機能のみ。実行は明示modeと専用unit等の条件を要求。通常のplan表示はkernel socketを作らない。製品adapterではない。
- [オフライン検証](../../scripts/nft-owner-probe-test.py): FakeSocketと模擬境界条件で15件。実Netlink／fork／SIGKILL／systemdは実行しない。

新規package・依存・権限付与・account・常設unitを追加していない。hp／Pi／cloudへの接続、host・network設定変更、実機試験は行っていない。既存のXDP・BFT・WALと証拠を保全した。

## 検証と限界

再現は次の二つ。いずれも非rootのMac上で実行するoffline検証であり、Linuxの成功を示さない。

```sh
python3 -I -B scripts/nft-owner-probe-test.py
python3 scripts/nft-boundary-offline-check.py
```

新probeの15件は、host namespace・余分な権限／NIC・module不在の拒否、固定owner tableだけのbatch、名前／sequence境界、ACK相関、再送なし、全family dump・終端必須・中断／上限／generation不一致の拒否、固定unitの制限、暗黙live modeの拒否を確認する。OSの認証・namespace・capabilityを実際に試した結果ではない。

初回13件の[出力](../../artifacts/nft-owner-prep-cjfpwd7h/python-tests.log)を保持した。レビューでsocket closeとprocess終了の混同を避けるため、close後も子を生存させる手順へ変更し、生存確認がPIDを回収しない検証とunit引数の検証を追加した。最終結果とhashは同directoryに別ファイルとして保存する。

最終検証では[Python15件](../../artifacts/nft-owner-prep-cjfpwd7h/python-tests-final.log)が成功。[plan・hash・保全結果](../../artifacts/nft-owner-prep-cjfpwd7h/result.json)はlive_executed=false、ban_apply_supported=false、production_preserved=true。実行せず出力した[unit引数案](../../artifacts/nft-owner-prep-cjfpwd7h/offline-plan.json)も保持する。独立Rustモデルの再回帰は[結果](../../artifacts/nft-boundary-check-px8hjvh1/result.json)に保存し、27件・format・Clippy・demo・apply拒否が成功。文書全体は[95ファイルのlint](../../artifacts/nft-owner-prep-cjfpwd7h/markdown-lint.log)で指摘0だった。

Linux一次資料との照合では、Netlink応答のnfgenmsg res_idにgenerationの下位16 bitが入る点を反映した。0固定として誤拒否しない。これはAPI照合であり、target kernelでparserが動作することの証明ではない。[table応答とgeneration](https://github.com/torvalds/linux/blob/v6.8/net/netfilter/nf_tables_api.c#L884)。

本番のRust source・workspace／lockfile・XDP object七ファイルは、今回開始時のhashと最終結果を照合する。新しいprobeが本体に組み込まれたとは扱わない。

## 残る作業と終了条件

| 作業 | 現在の状態／終わる条件 |
| --- | --- |
| owner限定の準備 | script・offline検証・対象／影響／復旧案を用意。実施承認には読み替えない |
| owner限定の実機確認 | 未実施。対象・影響・復旧への別承認後に一回。両終了case・対照・最終0・host保全が揃えばその確認を終了 |
| nftでのBAN | 保留。現行絶対期限との同等性が確認できなければ不採用。owner成功で解除しない |
| 公開pilot | HOLD。既存の[受入れ条件](../local-ban/DELIVERY_GATE.md)は未達で、今回のoffline成功で代替しない |

モデル／推論: 継続指定Sol／High。実行記録ではgpt-6.1-sol／high。分担なし。入力・cache入力（入力の内数）・出力の使用量は未取得。実機への追加課金試験はなし。
