# 固定送信元ポートによる限定WAN試験：開始前中止と復旧

2026-10-26 JST、09:35–09:42。最終ゴールは公開サイトで安全に操作・復旧できること。今回は、携帯回線からhpへのHTTPS到達後に限り、一件TTL30秒のBAN・明示解除へ進む予定だった。

**結果は未完了。WAN接続を開始する前に中止した。XDP装着・BAN・解除は未実施。転送off・一時サービス停止・非enabled・Wi-Fi復元と既存環境の保全は確認した。**

## 確認した範囲

- F660Pの現在のWAN IPv4と、既存試験転送のoffを画面で確認した。Pi test転送on／Pi main転送off／VPN転送on、全体防御「高」を維持した。
- 同一ハッシュの既存製品とobjectを、上書きなしの新しい一時領域へ配置した。製品コード・WAL・同期・TTL・合意条件は変更していない。
- Macの有線en0を管理経路に残し、Wi-Fiだけ承認範囲で一時DHCPへ変更。SDKの`IP_BOUND_IF`でWi-Fiに固定した。
- 同じローカルIPv4・TCP送信元ポートを用いて二つの外部IPv4へTLS接続し、一方を保持した。CA・hostname検証は有効。自動再接続なし。
- hpのHTTPSは別DynamicUserの一時unitで起動し、Macと当該携帯source一件だけを許可した。Macからの証明書・期待body検証と、許可外のhp自身からのTCP拒否、実cgroup filterを確認した。

公開IP・ポート・応答nonce等の実値はprivate証拠に残し、ここでは省略する。秘密鍵を取得・出力していない。

## 二つの試行を区別する

| 試行 | 観測・停止箇所 | 実施しなかったこと |
| --- | --- | --- |
| C | 20秒間隔の接続保持照会と上限12回を併用。承認後、限定転送を一時onにしたが、開始ゲートで残り照会4回・必要6回となり停止。残り時間は55.14秒で、時間だけの不足ではない | hpへのWAN接続、XDP装着、BAN、解除 |
| D | Cの記録を保全し、新しい一時領域で照会間隔を40秒へ変更。保存直前に最新IPを検証し、残り60秒以上・照会6回以上を要求する設計にした。新しいsourceの承認が有効時間内に届かず、下書きをキャンセルした | 転送onの保存、SYN観測、hpへのWAN接続、XDP装着、BAN、解除 |

Cは試験手順の不備だった。承認待ちにも照会枠を消費するのに、保存前には残り時間・回数を同時に予約していなかった。Dでは保存前のゲートを強め、全体180秒・照会12回を維持した。後から届いたDへの承認を、終了済み接続や新しいsourceへの承認として流用していない。

Cの反映IPは8照会・最初から最後まで約121.81秒、Dは5照会・約120.88秒の観測でそれぞれ同一だった。これは保持した照会先の観測であり、hp向けNAT mappingの一致を証明しない。接続を閉じたので、いずれのsourceも次回へ流用しない。

CのSYNヘッダー限定観測は、非promiscuous・対象IPv4/TCP18443・最大15秒／12件で終了し、捕捉0件、timeoutの終了コード124だった。**WAN接続要求自体を開始していないため、0件を回線・ルーターによる遮断の証拠にしてはいけない。**

## 復旧と保全

- Cの転送はoffへ保存し、再読込みで確認。Dの未保存下書きはキャンセルし、再読込み後もoffだった。既存三転送のon／off／onを維持した。
- C／Dともdaemonは起動していない。HTTPSを停止し、最終監査で両unitのPID0・inactive／not-found、残存試験processなし、18443／18444待受なし、常設unit・enable symlinkなしを確認した。
- すべての保持socketを閉じた。Wi-FiとEthernetの表示設定は元と一致し、hp・Pi管理経路はen0のまま。
- 独立したbefore／afterで、hpのboot ID、既存サービス、設定ファイルhash・属性、netplan・常設unit一覧、EFI、nft／iptables／ip6tables、BPF linkを照合した。差分はIPv6アドレス・経路の残存寿命だけで、これらの寿命欄を除いた構造は一致した。原JSONも保全した。
- Piのboot ID、Dockerの状態・PID、TCP待受一覧はbefore／afterで一致し、SSHも応答した。これはPiアプリケーション全機能の網羅検査ではない。
- 新規fixture・証拠・終了済み所有記録は削除せず残した。既存データの削除、host再起動、EFI・DNS・全体防御の変更はしていない。

## 判定と再開条件

公開受入れは引き続きHOLD。[以前のWAN接続失敗](2026-10-26-WAN-SOURCE-PREFLIGHT.md)とは停止箇所が違い、今回はWANでTCP timeoutを再現した結果ですらない。製品、NIC、携帯NAT、F660Pのどれが原因かは、この試行から判定できない。

同じ手順の反復は止める。再開前に、手動承認の待機と実WAN試験の制限時間を分離した進行手順を具体化する。承認待ち用の外向き接続枠を変更するなら、その上限・終了条件を別途確認し、製品のTTL・daemon180秒／HTTPS240秒・送信元一件の許可・WAN到達後のみBANする条件は維持する。待機枠の変更は今回は未実施。

再開時も、最新sourceの確認とその確定値の保存承認が必要。実HTTPSの証明書・body・hpが見たpeerの一致を確認してからXDPへ進む。自然失効・restart・独立した正常WAN clientの試験は、今回の一件BAN／解除計画にも含めていなかったため、公開受入れ全体の未検証事項として残る。

## 証拠と検査

- C：[結果・停止ゲート・サービス復旧](../../artifacts/hp-ubuntu-pinned-wan-k6434yqi/result.json)、[保持接続の原結果](../../artifacts/hp-ubuntu-pinned-wan-k6434yqi/pin-result.json)、[Wi-Fi復元](../../artifacts/hp-ubuntu-pinned-wan-k6434yqi/owner-result.json)、[限定SYN観測](../../artifacts/hp-ubuntu-pinned-wan-k6434yqi/syn-observer.json)。
- D：[未開始の結果](../../artifacts/hp-ubuntu-pinned-wan-reserved-_moxga2u/result.json)、[サービス停止監査](../../artifacts/hp-ubuntu-pinned-wan-reserved-_moxga2u/cancel-cleanup.json)、[保持接続の原結果](../../artifacts/hp-ubuntu-pinned-wan-reserved-_moxga2u/pin-result.json)、[Wi-Fi復元](../../artifacts/hp-ubuntu-pinned-wan-reserved-_moxga2u/owner-result.json)、[独立保全比較](../../artifacts/hp-ubuntu-pinned-wan-reserved-_moxga2u/preservation.json)。

試験用Pythonの構文検査、保持枠の回数予算の算定、実HTTPSと拒否、保存結果の再集計、終了状態とbefore／afterの独立照合を実施した。製品コードの変更や回帰試験の再実行はしていない。回数予算の算定を、WAN試験の成功や実行時ゲートの検証済みという扱いにはしない。
