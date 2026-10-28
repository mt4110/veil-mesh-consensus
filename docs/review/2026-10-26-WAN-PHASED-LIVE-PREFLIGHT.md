# 時間枠分離後の限定WAN予備試験：認証切れで中止・復旧完了

2026-10-26 JST、10:18–10:32。[時間枠分離のオフライン検証](2026-10-26-WAN-PHASES-OFFLINE-VERIFICATION.md)後、本人承認の範囲で限定試験を再開した。

**WAN試験は未完了。保存操作時にF660Pがログイン画面となり、保存結果を確認できなかったため中止した。WAN接続・SYN観測・XDP装着・BAN・解除は未実施。本人の再ログイン後に試験転送offを確認し、サービス停止・Wi-Fi復元・既存環境の保全まで完了した。**

最終ゴールは公開サイトで安全に操作・復旧できること。今回の結果は開始前の手順・復旧の実測であり、hpへのWAN到達性や公開サイトでのBAN成功ではない。

## 条件と対象

- 試験用転送`vhp-wan-20261025a`一件、現在の携帯source一件だけ、WAN TCP18443からhpの192.168.1.6:18443へ。具体値と復旧方法を画面・確認票で提示し、保存前に本人承認を得た。
- 新規一時対象は`/run/vhp-wan-20261026e/`と同prefixのHTTPS／daemon unit。既存と衝突しないことを確認し、製品／objectの同一hashを照合した。
- 待機600秒・20照会、開始準備30秒、実試験監視180秒・専用12照会、HTTPS240秒／daemon180秒を維持した。予定するBANは一件TTL30秒、容量256。
- MacのWi-Fiだけ承認範囲で一時DHCPへ変更。有線en0のhp／Pi管理経路、既存Pi／VPN転送、全体防御「高」、DNS・EFI・既存サービスを維持した。host再起動なし。
- live fixtureは`artifacts/hp-ubuntu-wan-phases-live-p8eifxtc/`。元のoffline fixtureは16ファイルのhash一致で保全を確認した。

公開IP・確認票の詳細・nonce等の実値はprivate証拠へ保存し、この文書では省略する。TLS秘密鍵は取得・出力していない。

## 進んだことと停止箇所

| 段階 | 実測・結果 |
| --- | --- |
| 承認待ち | 約255.58秒。待機照会8回。待機終了時に実試験枠は未消費で、承認前のhpサービス起動・WAN接続なし |
| 開始準備 | 承認後に同じ保持接続でsourceを再確認。source別fixture配置、HTTPS起動、実cgroup filterと許可外source拒否、LANの証明書／body・hp／Piの管理疎通を確認 |
| 監視開始 | 開始準備への遷移から約2.99秒で`arm`へ移り、新しい180秒期限と保存結果不明時の復旧義務を記録 |
| 保存操作 | browserが保存controlを確認できずtimeout。次の画面はF660Pのログイン画面。入力が届いたか・保存されたかは確認できず、再クリックしなかった |
| 中止 | `saved`確認を送らず、WAN接続・SYN観測・daemon起動・BANへ進まなかった。HTTPSを停止し、対象unit・process・portを監査 |
| 復旧完了 | 本人の再ログイン後に新しく読み込んだ画面で転送offを確認。offへの追加保存は不要だった。既存三転送on／off／onと防御「高」も確認 |

上記時間はMacのmonotonic clockによる当該試行の観測であり、性能改善率や上限達成の保証ではない。照会先でのsourceは約295秒にわたる10応答で同一だったが、hp向けNAT mappingは依然未確認。

**以前の共有180秒・12照会による中止は再発していない。今回は認証の確認方法が不十分だった。** 承認後に画面上の設定値を読んだが、残表示だけではサーバー側sessionの有効性を確認できなかった。F660Pの認証有効期間や、特定操作による延長は測定していない。

## 復旧時に見つけた試験helperの不備

最初に担当が正しい空JSON入力で`stop-daemon`・`stop-https`・`final`を実行し、PID0・inactive、残存processなし・試験待受なし・常設unit／enable symlinkなしを確認した。

一方、親の自動fallbackはstdinに空JSONを渡しておらず、root helperの`json.load`で拒否された。原結果は終了失敗のまま保全した。以前のオフラインfakeはprocess境界のstdin契約を検査しておらず、この不備を見逃していた。

Wi-Fi復元commandは一度だけ実行され、終了0だった。直後の読取りでは元の手入力付きDHCP・192.168.1.9へ戻っている一方、携帯側の旧mask／gateway欄が残り、元表示との完全一致判定は失敗した。その後の独立確認では元表示と一致した。設定変更は再送していない。

本人の再ログインが復旧30秒枠を越えたため、pinとownerの原結果も復旧未確認のまま終了した。後から原結果を書き換えず、独立した最終確認を`summary.json`へ追加し、**試験未完了と復旧完了**を分けて記録した。

## 最小修正と検証

新しいprivateコピー`artifacts/hp-ubuntu-wan-recovery-fix-zsew_vwh/`だけで次を修正し、hostへ再配置・WAN再試験はしていない。

1. 親のfallbackが空JSON`{}`をstdinへ渡すように修正。process呼出しまでの入力契約を検査する回帰を追加し、修正前の失敗を再現して保存した。
2. 復元commandが成功した後、古いDHCP情報が残る場合だけ最大2秒の読取り確認を追加。設定変更を繰り返さず、親900秒の外側上限内で処理する。サービス・WAN試験・TTLの期限は延長しない。
3. 読取りが収束する場合と収束しない場合を検査。収束しない場合は復旧未完了とし、並行するWi-Fi変更を上書きしない。

外部操作を拒否するaudit guardとfake clock／adapterによるオフライン30項目が成功。これは実際の親fallback修正版をhpと接続して確認した結果ではない。元の28項目が通っただけでは実際のprocess間契約を網羅できていなかったことを、今回の制約として残す。

## 最終状態と保全

- 試験転送off。既存Pi test on／Pi main off／VPN on、防御「高」を認証後の画面で確認した。
- HTTPS／daemonとも停止・非enabled。試験待受・残存process・所有XDPなし。daemonは一度も起動していない。
- 保持socket終了、MacのWi-Fi・Ethernetの表示設定は変更前と一致。hp／Pi管理経路はen0のまま。
- hpのboot ID、既存サービス、設定file hash・属性、netplan・常設unit一覧、EFI、nft／iptables／ip6tables、BPF linkはbefore／afterで一致。IPv6アドレス・経路の差は残存寿命だけで、原JSONとその比較を保存した。
- Piのboot ID、Dockerの状態・PID、TCP待受はbefore／afterで一致し、SSHは応答した。Piアプリケーション全機能や完全無瞬断の証明ではない。
- fixture・失敗記録・所有記録・TLS credentialは削除せず保全。旧データ・hostの永続設定・既存サービス設定の変更なし。

## 次回へ反映する認証手順

本人から、作業中に左上ZTEを押す操作を提案された。実際に押すと認証済みのホーム画面へ戻ることを確認したが、sessionの延長効果は保証しない。常設の自動操作や無期限の保持は作らない。

承認待ち中は、作業中のbrowserでおおむね1分ごとにZTE／ホームを開く。その際、未保存下書きが失われることを前提にする。実値は確認票へ保持し、承認後に同じ値を新しい下書きへ反映する。保持接続のrunとsourceを変えず、値が違えば中止する。

**`begin`の前、まだ承認待ち枠の間に**ホームから転送画面を新しく読み込む。認証が切れていれば、この段階で本人の再ログインを待つ。承認済みの下書きを再度照合してから開始準備へ進み、残表示の読取りだけを`router_authenticated=true`の根拠にしない。`arm`前にも認証切れを検知したら、保存・WAN接続へ進まず復旧する。

今回の保存承認は終了したrunに属する。再ログインへの回答を、新しいsourceや新しいWAN試験への承認として再利用しない。同じ接続・配置先での再試行は行わず、公開受入れHOLDを維持する。

## 証拠

- [実試行の最終集約](../../artifacts/hp-ubuntu-wan-phases-live-p8eifxtc/summary.json)、[初期復旧監査](../../artifacts/hp-ubuntu-wan-phases-live-p8eifxtc/save-unknown-recovery.json)、[browser観測](../../artifacts/hp-ubuntu-wan-phases-live-p8eifxtc/browser-save-observation.json)。
- [pinの原結果](../../artifacts/hp-ubuntu-wan-phases-live-p8eifxtc/pin-result.json)、[親の失敗を含む原結果](../../artifacts/hp-ubuntu-wan-phases-live-p8eifxtc/owner-result.json)、[再ログイン後の画面確認記録](../../artifacts/hp-ubuntu-wan-phases-live-p8eifxtc/router-final.json)、[独立したMac復元確認](../../artifacts/hp-ubuntu-wan-phases-live-p8eifxtc/mac-final.json)、[hp／Pi保全比較](../../artifacts/hp-ubuntu-wan-phases-live-p8eifxtc/preservation.json)。
- [fallback修正前の回帰失敗](../../artifacts/hp-ubuntu-wan-recovery-fix-zsew_vwh/regression-before.txt)、[修正後の30項目](../../artifacts/hp-ubuntu-wan-recovery-fix-zsew_vwh/regression-after.txt)、[修正版の親helper](../../artifacts/hp-ubuntu-wan-recovery-fix-zsew_vwh/owner.py)、[検証コード](../../artifacts/hp-ubuntu-wan-recovery-fix-zsew_vwh/offline_tests.py)。
