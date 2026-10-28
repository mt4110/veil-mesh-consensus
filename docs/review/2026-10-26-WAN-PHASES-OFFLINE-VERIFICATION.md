# 限定WAN試験helper：時間枠分離の実装とオフライン検証

2026-10-26 JST。[変更案](2026-10-26-WAN-APPROVAL-AND-TRIAL-PLAN.md)の本人承認後、試験用helperの改修とオフライン検証までを実施した。

**後続記録:** [実機予備試験と復旧](2026-10-26-WAN-PHASED-LIVE-PREFLIGHT.md)では時間枠の分離を確認したが、保存時の認証切れで中止した。親fallbackのstdin契約に抜けが見つかり、元の証拠を保全した別コピーで修正・回帰検証した。以下の28項目は当時の検証範囲で、実機の全動作を保証しない。

**今回の完了条件は達成。承認待ちと実試験の期限・照会枠・サービス起動を分離し、28項目のオフライン検証が成功した。ネットワーク設定変更、SSH操作、外部接続、WAN試験、XDP装着は行っていない。公開受入れは引き続きHOLD。**

最終ゴールは公開サイトで安全に操作・復旧できること。今回の成功は、その前段の試験手順の検証であり、携帯NAT／F660Pの到達性や公開サイトでのBAN成功を示さない。

## 変更範囲

新しいprivate fixtureは`artifacts/hp-ubuntu-wan-phases-uo4y2w0a/`。製品ソース・既存実験helperを編集せず、前回のhelperを新規対象用に複製して改修した。新しい依存はなく、Python標準ライブラリを使う。

| ファイル | 変更内容 |
| --- | --- |
| `phases.py` | 共通の期限・照会数・状態・承認scopeの管理。WAIT→START→RUN→CLEANUP→FINISHEDの遷移を制限 |
| `pin.py` | 同じ保持socketを使い、待機600秒／20照会と承認後12照会を分離。開始準備30秒、保存直前の実試験監視180秒、復旧30秒を独立して開始 |
| `ipc.py` | 命令をrun IDと確認票digestに結び付け、後着・別run・終了済みrunの利用を拒否。結果不明を自動再送しない |
| `bind-source.py` | 旧待機枠の残り70秒という条件を廃止。承認後のSTART状態・source・run・scopeを照合して新規配置する |
| `trial.py` | `prepare`で承認後のHTTPS起動・制限・LAN／管理検査、`run`で保存確認後のWAN・BAN・解除を実施する形に分離。保存結果不明の復旧義務を保持 |
| `owner.py` | 外側900秒、子の最大840秒と復旧用余白を管理。子終了後の所有unit停止・監査、Wi-Fi復元、結果集約を担当 |

既存のroot helper・HTTPS・unit templateは新しい名前へ読み替えたものを使用し、サービスの設定は維持した。送信元は単一IPv4、WANはTCP18443、宛先192.168.1.6:18443、BANのTTL30秒・容量256、daemon180秒／HTTPS240秒・Restart=no・停止猶予10秒で固定している。再起動でBANが消える製品の契約を変更していない。

完了判定も分けた。試験の機能確認が成功しても`trial.py`は全体を完了扱いにしない。保持接続の終了、転送offの確認、実unit・XDP・待受、Wi-Fiと管理経路の復元を集約して初めて全体の成功を判定する。承認なしで待機だけ終了した場合は、復元成功と機能未実施を区別する。

各段階の再実行では既存結果を上書きして進めず、既存／実行中のrunを保全して拒否する。新しいfixtureの`plan.json`は`live_client_authorized=false`、`wan_ip=null`のまま。五つの外部操作用entrypointは、この状態で実行しても外部操作の前に拒否する。

## オフラインで確認したこと

テストはfake clockと偽の通信・process・サービスadapterを使用した。加えて、テストprocess内のPython audit hookでsocket作成・接続・名前解決・process起動等を拒否し、ガードが実際に検知することも検査した。これは外部通信の実測ではなく、テスト実行中に実アダプターへ抜けないための検査である。

| 検証対象 | 結果 |
| --- | --- |
| 承認待ち0／120／300／599秒 | 承認後の新しい期限と未消費の実試験枠を使用。実際の`pin.py`ループでも5回の偽WAN接続と終了まで進行 |
| 古い待機期限の通過 | 599秒で承認後、600秒を越えて開始準備・実試験を継続。旧期限を理由に中止しない |
| 期限・回数の境界 | WAIT600秒、START30秒、RUN180秒、CLEANUP30秒と照会上限を検査。再命令で期限や回数をリセットしない |
| 承認・source・保持接続の不一致 | 別run、異なるscope、source・local port・保持FDの変化、認証未確認、後着命令を拒否 |
| WAN前の準備 | 承認前にHTTPS／daemonを起動しない。承認後の準備でもWANへ接続せず、保存確認前のprobeを拒否 |
| WANの接続失敗 | 機能試験は最初の接続失敗で終了し、XDP起動・BAN・自動再試行をしない |
| BANの結果不明 | 一度の要求後、再送・解除への推測進行をせず、所有サービスの停止・監査へ進む |
| HTTPS起動結果不明・filter不備 | WANへ進まず復旧。最終監査失敗は成功にしない |
| 子process強制終了相当・証拠破損 | 残存サービスの可能性を保守的に扱い、停止・Wi-Fi復元を試みる。確認できない結果は手動復旧必要として残す |
| 並行するWi-Fi変更 | 自分の変更と一致しない状態を上書きせず、復旧未完了として報告 |
| 親の900秒上限 | 呼出しtimeoutを残り時間へ制限し、期限後は新しい外部操作を開始しない |
| 再実行・配置先衝突 | 既存結果・fileを保全して拒否。二度目の起動／転送要求を送らない |
| 固定条件と安全装置 | TTL・容量・対象・hash・サービス期限の変更を拒否し、渡したoffline設定で外部entrypointが拒否される |

最終結果は28テストメソッド成功。方法数は品質や実環境の証明ではなく、上表の条件と原結果を判断材料とする。

再実行拒否の検査を加えた途中の一回は、`pin.py`内のローカル関数名が共通guardを隠していたため失敗した。名前の重複を除去して修正し、期待値・安全条件は変更せず最終検査を通した。失敗時の出力も保全している。

## 保全と未実施事項

前回C／D fixtureの107ファイルについて、改修前後のSHA-256がすべて一致した。製品・objectや既存証拠を上書き・削除していない。検証用の新規directory・偽の応答・失敗記録も削除せず残した。

今回、Wi-Fi状態の読取りも含め実機のネットワーク操作はしていない。SSH・ルーターUI・systemdへ接続しておらず、Pi・hpの現時点の稼働状態を再検証した結果ではない。

次は依然として未確認である。

- 実際の10分枠で照会先と携帯回線が保持接続を維持するか。切断時の自動再接続は実装していない。
- 照会先で観測したIPをhp向けNATでも使うか、F660Pの限定転送を通過するか。
- 実時計・実SSH・GUIの遅延やprocess間競合、macOS／Linuxの実障害時に復旧が完了するか。
- WANでのBAN・解除・期限失効・restart、独立した正常WAN clientへの影響。

共用F660Pの保存時にPiが無瞬断となる保証も得ていない。実施時にPiの管理・既存サービスへ変化があれば中止する。完全無瞬断が必須なら、ルーターを変更するWAN試験へは進まない。

## 次回の実行順と承認境界

今回はここで止める。実行の承認を受けた場合だけ、以下を一続きで準備する。

1. 新鮮なMac／hp／routerのbaselineとWAN値を読み、新規一時配置・製品hash・復旧手順を確認する。今回のoffline fixtureを有効化するだけで旧観測値を流用しない。
2. Macの保持接続を開始し、現在の確認票を提示する。routerはoff、hpの試験サービスは停止したまま、本人の保存承認とrouter認証を待つ。
3. 同じ確認票への明示承認と認証を確認して`begin`を一度だけ送る。`bind-source.py`と`trial.py prepare`を直列に進める。ここで人の追加回答やコード編集を待たない。
4. 未保存下書きが承認値と一致することを画面で確認し、`arm`で監視・復旧義務を記録してから保存する。再読込みで確認できた場合だけ`saved`を送り、`trial.py run`へ進む。GUI操作とその確認をhelperが代行したとは扱わない。
5. 試験終了後、所有サービス停止・監査、試験転送offの再読込み確認を済ませてから、`finish`に復旧証拠を渡す。親のWi-Fi／管理経路の確認と結果集約まで完了を見る。

保存応答不明でも転送offの義務を残す。復旧不能は完了扱いにせず、該当する新規unitだけの停止・転送一件のoff・元Wi-Fi方式への復元を本人へ引き継ぐ。各時間制限は操作の期限やサービス側上限であり、GUIが利用不能でも180秒以内に転送offを保証するものではない。

## 成果物と再現

- [状態・期限の共通処理](../../artifacts/hp-ubuntu-wan-phases-uo4y2w0a/phases.py)、[保持接続](../../artifacts/hp-ubuntu-wan-phases-uo4y2w0a/pin.py)、[親の復旧・集約](../../artifacts/hp-ubuntu-wan-phases-uo4y2w0a/owner.py)。
- [IPC](../../artifacts/hp-ubuntu-wan-phases-uo4y2w0a/ipc.py)、[source配置](../../artifacts/hp-ubuntu-wan-phases-uo4y2w0a/bind-source.py)、[開始準備・機能試験](../../artifacts/hp-ubuntu-wan-phases-uo4y2w0a/trial.py)。各旧版との差分もfixture内に保存。
- [オフライン検証コード](../../artifacts/hp-ubuntu-wan-phases-uo4y2w0a/offline_tests.py)、[最終原出力](../../artifacts/hp-ubuntu-wan-phases-uo4y2w0a/offline-test-results-final.txt)、[最終検証票](../../artifacts/hp-ubuntu-wan-phases-uo4y2w0a/verification-final.json)、[旧証拠の比較](../../artifacts/hp-ubuntu-wan-phases-uo4y2w0a/preservation.json)。

ネットワークを使わない再現commandは次の一つ。テスト用directoryはprivate fixture内へ新規作成し、削除しない。

```bash
python3 -I artifacts/hp-ubuntu-wan-phases-uo4y2w0a/offline_tests.py
```

実際の接続・証明書・GUI・サービス・XDPの受入れはこのcommandの対象外。公開試験導入の合格欄は未チェックのままとする。
