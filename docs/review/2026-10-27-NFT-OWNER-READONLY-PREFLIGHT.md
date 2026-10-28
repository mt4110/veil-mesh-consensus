# hp-ubuntuのowner限定試験：読取り事前確認

日付: 2026-10-27 JST。本人承認は**hp-ubuntuの読取り確認まで**。試験用の配置・unit起動・namespace作成・table作成・設定変更は行っていない。

最終ゴールは小規模公開サイトで安全に使える手動一時BAN。現在のマイルストーンはnftables候補の停止・回収機能の評価。今回のゴールは、実機の前提と既存状態を読取り、隔離試験の対象・影響・復旧案と停止条件を確定すること。

## 結果

**hostの基礎条件は確認できたが、準備済みscriptのPID 1 namespace参照と最小権限の組合せに問題がある。このままの実行は保留。権限を増やさず、事前参照情報を渡す最小修正・offline検証を先に行う案とする。修正・実機試験は今回未実施。**

後続の本人承認で[事前参照への置換・24件のoffline回帰](2026-10-27-NFT-OWNER-HOST-REFERENCE.md)を実施した。以下は読取り時点の記録として保全する。後続でも実機への配置・隔離試験は未実施。

| 読取り対象 | 確認結果 |
| --- | --- |
| 接続・sudo | ControlMasterを使わない新規SSH成功。`sudo -n true`成功 |
| OS／kernel | Ubuntu 26.04.1 LTS、x86_64、7.0.0-38-generic |
| 起動時刻 | `uptime -s`は2026-10-26 05:08:28。この確認で再起動していない |
| tool | Python 3.14.4、systemd 259.9、nftables 1.1.6。ip・systemd-runも既存配置 |
| namespace基盤 | cgroup v2。container検出はnone。rootからselfとPID 1のnet／pid／user namespaceが一致 |
| Netfilter | nf_tablesは事前ロード済み。hostのnft一覧はtable／chain／rule 0件。legacy table名のprocファイルは不在 |
| UFW | ufw.serviceはactiveだが、`ufw status`はinactive。service起動状態と防御有効は別 |
| NIC | eno1はUP／LOWER_UP、MTU1500。取得した詳細linkにXDP情報なし |
| 待受 | 選択した22／80／443／18443の確認では22だけ待受。全portの安全監査ではない |
| 試験対象の衝突 | `veil-nft-owner-probe-20261027a.service`はnot-found／inactive、対応`/run`配置先は不存在 |
| 資源 | `/run`空きは取得時に約13.0GB。将来の実行時の空きを保証しない |

UFW無効・nft空という読取りから、routerや別の防御まで無効と判断しない。router・Pi・cloudは照会していない。公開サイトや既存防御の受入れ判定にも使わない。

## 読取りで見つけたscriptの前提

通常ユーザーの補足収集で`os.stat('/proc/1/ns/net')`がPermissionErrorとなった。これは**準備済みprobeを実行した結果ではない**。probeのroot＋CAP_NET_ADMINだけの状態も、今回は実行していない。

rootによる追加の読取りでは、PID 1のCapPrmは`000001ffffffffff`だった。probeの計画値はCAP_NET_ADMINだけ（`0x1000`）。Linuxのnamespace参照はPTRACE_MODE_READ_FSCREDS検査を受け、共通のcapability検査では対象の許可capabilityを包含するか、CAP_SYS_PTRACEが必要となる。このため、現在のscriptがPID 1を直接参照できると仮定した点を修正対象とする。[namespace参照の権限](https://man7.org/linux/man-pages/man7/namespaces.7.html)・[capabilityの検査](https://man7.org/linux/man-pages/man2/ptrace.2.html)。

元の15件は模擬factsと応答parserのoffline検証であり、実OSのこのアクセス条件を検証していなかった。失敗を成功へ変更したり、CAP_SYS_PTRACE／CAP_SYS_ADMINを追加して通したりしない。

### 最小修正案・未実装

1. 将来の承認後、配置前の読取りでPID 1のnet／pid／user namespace識別値とboot IDを取得する。
2. 承認された新規root0700 directoryに、root所有0400の固定`host-reference.json`として新規保存する。未知ファイルは上書き・削除しない。
3. probeは参照ファイルの固定path・所有者・mode・通常ファイル・symlink拒否・サイズ／形式を検証する。同じboot IDであることとself namespaceを照合し、host net namespaceを拒否、pid／user namespaceの一致を要求する。別namespaceへ入るFDを受け渡さない。
4. PID 1へ直接アクセスする前提を取り除く。参照不正・別boot・host namespace・権限不足の拒否をoffline検証し、script hashとunit案を更新する。
5. 固定したscript・追加参照ファイル・unit・停止手順を提示してから、一回の実機試験の承認を得る。

rootによる事前記録を信頼する案であり、侵害されたrootへの防御ではない。修正後のOS動作も未検証。実機試験に進める状態へ修正できなければ、そこで停止する。

## 保全の照合と失敗記録

private evidenceは[事前確認directory](../../artifacts/nft-owner-preflight-mbje_ks1/)に保存した。生のIP・経路・namespace情報を含むため、公開用の結果とは分ける。

- [初回読取り](../../artifacts/nft-owner-preflight-mbje_ks1/initial.json)、[tool・firewall・service](../../artifacts/nft-owner-preflight-mbje_ks1/detail.json)、[rootによる補足読取り](../../artifacts/nft-owner-preflight-mbje_ks1/root-read.json)。
- [前後比較](../../artifacts/nft-owner-preflight-mbje_ks1/summary.json)ではlink、nft一覧、稼働service名が一致。address／routeの生JSONは完全一致ではない。
- [差分の全項目](../../artifacts/nft-owner-preflight-mbje_ks1/network-differences.json)はvalid_life_time、preferred_life_time、expiresだけ。残時間の減少に加え一つのexpiresが更新されている。値を伏せて完全一致と報告せず、address・routeのその他の取得項目に差がないと区別する。
- `systemd --version`はPATH上にbinaryがなく失敗し、既存`systemctl --version`で確認した。通常ユーザーによるPID 1参照の失敗も[stderr](../../artifacts/nft-owner-preflight-mbje_ks1/final-ssh.stderr)へ保全。未知の試験状態やowner回収失敗とは混同しない。

実施したsudoはtrue、nft一覧、ufw status、固定したPython読取りのみ。既存serviceの停止・起動、設定書込み、package導入、moduleロード、capability変更、host再起動はしていない。SSH／sudoの通常の監査記録等がOSに生じることは、設定変更とは別である。全既存データのhash監査を行ったという意味ではない。

## 将来の変更対象・影響・復旧案

| 区分 | 修正後に承認を求める予定の範囲 |
| --- | --- |
| 新規配置 | hpの`/run/veil-nft-owner-probe-20261027a/`、script、root所有0400のhost-reference.json、0700のevidence。今回は未配置 |
| 一時unit | `veil-nft-owner-probe-20261027a.service`。PrivateNetwork、loだけ、CAP_NET_ADMINだけ、60秒・停止猶予5秒、Restart=no |
| kernel変更 | 専用namespaceにownerの空table最大2個。chain／rule／set／hookなし。socket closeと自分の子のSIGKILLによるtable回収を含む |
| 影響 | host・Piの防火壁／route／物理NICへ変更しない。共有kernelの資源・不具合の残余リスクはある |
| 復旧 | 所有確認した今回unitだけ停止。専用process／socket／namespaceの参照解放と、新規SSH・既存状態を確認。host全体のflush・再起動・未知table削除はしない |
| 終了 | 証拠を退避し、unit停止・非enabled。新規ファイルは削除せず保全。未確認の回収を成功としない |

詳細は[隔離試験案](../local-ban/NFT_OWNER_PROBE_PLAN.md)。今回の読取り承認を、上表の実行承認には読み替えない。

**残り:** 参照方法の最小修正＋offline検証 → 対象・影響・復旧への承認 → owner限定の隔離実行一回。ownerの確認が終わっても、絶対期限の同等性とBAN適用は未達のまま。公開pilotはHOLDを維持する。

モデル／推論は現在sessionの実行記録でgpt-6.1-sol／high。分担なし。今回の入力・cache入力（入力の内数）・出力の使用量は未取得。追加課金試験なし。
