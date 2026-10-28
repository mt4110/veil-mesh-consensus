# 試験導入前チェックリストと完了条件

[README](../../README.md) · [導入](INSTALL.md) · [操作](OPERATIONS.md) · [復旧](RECOVERY.md)

**現在の判定は公開pilot準備中／配備HOLD。** 以下は記入用の票で、チェック済みの承認票ではありません。未確認を推定で埋めず、根拠・確認者・日付を残します。
最終ゴールは実サイトで安全に運用・復旧できるOSS。試験導入の技術的成功と、継続採用する価値は別に判断します。

今回の製品ゴールと、合格に必要な六条件、自宅最大一回からVMへ進む順序、試験打ち切りと合格の違いは[ゴール・受入れ票](DELIVERY_GATE.md)へ固定した。GCPの作成Aと別zone一回比較は承認・実施済みだが、下記の作成失敗で停止した。公開Bや追加の作成はその承認と分ける。

## 再開位置（2026-10-26）

最新の[要求・監査ログ分析と残り四工程](../review/2026-10-26-GCP-503-REQUEST-ANALYSIS.md)を参照。三要求のcode14と取得項目の整合を確認したが、根本原因は未確定。追加作成なし、公開B保留。現要求の安全条件を外して通す修正は採用しない。

**最新の再開位置:** 携帯テザリングは以後使わず、自宅の追加WAN試験は保留。[二台の構成案](GCP_TWO_VM_EXECUTION_PLAN.md)のA失敗後、[承認済みus-central1-b一回比較もVM作成503、新設network等は撤去・保全照合まで完了](../review/2026-10-26-GCP-A-ZONE-B-COMPARISON.md)。追加の作成・zone変更・時間枠の再開始は行わない。以下の自宅経過は履歴として保全し、次の再開指示として扱わない。専用FQDNも未変更で、公開受入れ欄は未チェックのまま。

ケーブル復旧後の限定WAN予備試験は、携帯側からTCP接続できずXDP前に停止した。携帯sourceの短時間変動も観測し、追加診断は保存前不一致で未実施。転送off・試験unit停止・非enabled・XDPなし・両SSH・Wi-Fi復元を確認。次の再開条件は、試験中の外向きIPv4を安定して確認できる外部試験元と、実入口のTCP到達・peer一致。[失敗と復旧の記録](../review/2026-10-26-WAN-SOURCE-PREFLIGHT.md)を使い、許可範囲や期限を広げて合格へしない。

外部サーバーを借りない候補として、[固定送信元portと接続保持の適合確認](../review/2026-10-26-EXISTING-ENV-WAN-OPTIONS.md)を設計した。初段はMacから既存HTTPS照会先への外向き確認だけで、Pi／router変更なし。ローカルsocketの成立だけをWAN合格へ転記せず、NAT適合とhpが見た実peerを段階的に確認する。

その後の[Macだけの実測](../review/2026-10-26-MOBILE-FIXED-SOURCE-PORT-MEASUREMENT.md)では、保持した同じ送信元endpointから二つの外部IPv4への12応答で、約50秒同じ外向きIPv4を確認し、Wi-Fi復元も確認した。hp向けの実peerとWAN入口は未検証。測定終了で接続を閉じているため、観測IPを次回へ流用しない。公開受入れHOLDは維持する。

続く[固定ポートによる限定WAN試験の記録](../review/2026-10-26-PINNED-WAN-TRIAL-HOLD.md)では、照会先では約121秒同一sourceを観測したが、試験用照会枠の不足と保存承認の有効時間切れで、WAN接続開始前に中止した。XDP／BANは未実施。転送off・サービス停止・非enabled・Wi-Fi復元・既存環境の保全を確認した。保持接続は終了済みで、後着の承認や観測IPを再利用しない。同じ段取りを繰り返さず、承認待ちと実試験の時間枠を整理してから再開する。

[承認待ちと実試験を分ける改訂案](../review/2026-10-26-WAN-APPROVAL-AND-TRIAL-PLAN.md)への本人承認後、[新規helperとオフライン検証](../review/2026-10-26-WAN-PHASES-OFFLINE-VERIFICATION.md)まで完了した。待機専用600秒／20照会と実試験用12照会を分離し、HTTPS240秒・daemon180秒・BANのTTL30秒を維持した。28項目の検証は偽時計・偽通信等であり、実機WANの成功ではない。当該offline fixtureは外部操作無効のまま保全し、この段階ではネットワーク変更・WAN試験を実施しなかった。実機再開は次段の記録と区別する。

その後の[改修手順による実機予備試験](../review/2026-10-26-WAN-PHASED-LIVE-PREFLIGHT.md)では、約256秒の承認待ちを実試験枠と分離できたが、保存時のrouter認証切れで中止した。WAN接続・XDP・BANは未実施。再ログイン後の転送off・サービス停止・Wi-Fi復元・保全を確認済み。親fallbackのJSON入力漏れも見つけ、別コピーのオフライン回帰で修正した。次回はこの修正版を使い、承認待ち中のホーム操作と、`begin`前の新しい転送画面の読み込みで認証を確認する。旧承認・旧sourceは再利用しない。

**机上での6場面、hp-ubuntuの隔離XDP、物理eno1の一件LAN受入れは完了。公開配備はHOLDのまま。** [本人の回答で確認した範囲](OPERATIONS.md#7-机上確認の記録2026-10-25)、[隔離実測](../review/2026-10-25-HP-UBUNTU-ISOLATED-XDP-ACCEPTANCE.md)、[物理NICの実測](../review/2026-10-25-HP-UBUNTU-PHYSICAL-NIC-ACCEPTANCE.md)を区別する。
Windowsを保全しUbuntuをメインに使うdual boot方針を維持する。Ubuntuは外付けUSB上。承認した一回のhost再起動後、Ubuntu先頭の起動項目と`ssh hp-ubuntu`・sudo復帰を確認した。公開経路は未確認で、この記録が追加の設定変更・再起動を承認するものではない。

[計画](../review/2026-10-25-HP-UBUNTU-PHYSICAL-NIC-PLAN.md)への承認後、eno1/e1000eでGeneric XDPの一件遮断・期限前の明示解除・独立正常client・新規SSH・対照port・停止後復帰を確認した。終了時はXDP・待受・試験processなし、非enabled。追加の隔離試験での操作process喪失・180秒自動detachとは別の証拠である。下の公開受入れ欄は実公開構成が未確定のため未チェックのまま。

環境準備後は、次の順で再開する。

1. **対象を連絡・記入する:** 新hostのOS／接続先、NIC、外部からUbuntuまでの経路、復旧consoleの有無を対象票へ記入する。まず読取りで実態を確認し、未確定値を埋めて配備しない。
2. **経路への適合を判断する:** XDP位置の実送信元と宛先IPv4／TCP portを確認する。CDN／L7 proxy等で利用者IPが見えなければ、利用者向けのこのprofileは保留／不採用とし、安易な機能追加へ進まない。防壁全体の完成ではなく、試験経路と管理・復旧経路が安定したことを再開条件とする。
3. **試験案を具体化して承認する:** 下のAに従ってbinary／object、account／unit／control-dir、保護source、影響、rollbackを確定する。既存pi-nodeの隔離成功を新hostへ転記しない。
4. **LANの本人操作へ進む:** 自動化した隔離・LAN受入れの証拠をもとに、本人が一覧・preview・明示適用／解除・停止を操作する。実機で迷った箇所・所要時間・補助の有無・通信と復旧結果を記録する。今回の自動試験を本人の無援助操作の合格にしない。
5. **公開は別の承認後:** 専用FQDN／TLS、canaryと独立正常client、時間帯・受入基準を確定してCへ進む。終了は停止・非enabled、XDP非装着と通信復帰まで確認する。

最初の再開に必要な入力はhostへの接続先と、試験経路／管理経路の説明。公開FQDN等は公開前に確定すればよく、未確定のまま公開へ進まない。
それまでは追加実装・同条件の実機再試験を積み増さない。再開後に具体的な欠陥や操作上の問題が出た場合だけ、根拠と必要範囲を示して次の変更を判断する。

| 残る未検証事項 | 机上確認から転記してはいけない結果 |
| --- | --- |
| 本人による実機操作・復旧 | 無援助での完了、実コマンドの入力／誤操作、所要時間、実通信確認、異常時の停止・固定recovery切替 |
| 新host／物理NIC | hp-ubuntuの選んだ成果物・権限・隔離vethと、eno1/e1000e Genericの限定LAN一件は確認済み。別NIC・native mode・負荷性能・別scopeは未確認 |
| 新ネットワーク／公開site | 送信元の見え方、NAT／proxy経路、TLS、保護source、正常clientと管理通信への影響 |
| 最新binaryで未実施の異常系 | 以前のbinaryでの所有記録破損・boot等の記録と区別。結果不明caseの実XDP発生も未確認 |
| 継続採用の価値 | 実際の対処場面、基礎防御との差、正常通信への影響、手動運用の負担、dual boot中の可用性 |

下のA・B・Cは実際の導入構成で確認する欄なので未チェックのまま残す。机上確認と新hostの隔離確認は、公開試験導入や継続採用の合格を意味しない。

## 対象票：空欄なら配備しない

| 項目 | 記入欄 |
| --- | --- |
| host／OS／kernel／source版 | 未確認 |
| binary／C objectのpath・SHA-256 | 未確認 |
| 実ingress NIC／driver／既存XDP・TC | 未確認 |
| XDP位置での宛先IPv4・TCP port | 未確認 |
| 専用FQDN／TLS終端／証明書の確認方法 | 未確認 |
| router／NAT／CDN／proxyの経路と実L3送信元 | 未確認 |
| 管理user／service UID／control-dir／unit名 | 未確認 |
| BAN禁止の管理・監視source IPv4 | 未確認 |
| 所有canary sourceと独立正常client | 未確認 |
| 基礎firewall／HTTP制限／対象外通信 | 未確認 |
| console担当／中止判断者／復旧手順 | 未確認 |
| 試験時間帯／終了時刻／終了時の非enabled確認 | 未合意 |
| baseline・入力条件・測定時間・受入予算 | 未合意 |
| 変更承認の対象・影響・rollback・記録 | 未承認 |

## A. 変更承認前

- [ ] 試験の目的は、既知sourceへの有人の一時対処と、その安全な解除・停止に限定した。
- [ ] 自動攻撃検知や無人防御、全protocol遮断、帯域飽和対策が必要なら、現profileがそれを満たさないと理解した。
- [ ] root／service UID侵害、共有NAT利用者の巻込み、単一IP／port上の他siteへの影響を確認した。
- [ ] IPv6／UDP443・HTTP3／VLAN／fragment等をVeilの保護済みに数えていない。必要な防御は基礎防御側で確認した。
- [ ] 実送信元が見えないCDN／L7 proxyを利用者の代わりにBANしない。既存CDNをこの機能のためだけに外さない。
- [ ] OS／disk／LAN変更は別の承認範囲。Windowsを残すdual boot方針と可用性の限界を反映した。
- [ ] 実scope・NIC・保護source・既存XDP・control-dir・復旧経路を確定した。
- [ ] binary／object、専用account、配置先、unit／drop-in、directory所有権の変更と戻し方を具体化した。
- [ ] DNS／router／FW／Web／TLSの変更があれば差分を示し、既存serviceを巻き込まない承認を得た。
- [ ] 初回はenableせず、終了時に停止・非enabled、XDPと通信復帰まで確認すると決めた。

## B. 公開前のhost／LAN受入れ

- [ ] 選択したbinary／objectの実体とhashを新hostで照合した。別build・旧binaryの合格を転記していない。
- [ ] 隔離環境で正常通信、256／257件、kernel期限、解除、古いinstance／revision拒否、正常／SIGKILL restartを確認した。
- [ ] 実UIDと全threadのEff／Prm／Inh／Ambゼロ・NoNewPrivs、実network namespaceを確認した。
- [ ] pending／Map error／応答不明を成功や空一覧に偽装しないこと、保護IP・TTL0拒否を確認した。
- [ ] 実NICのLAN試験で空BAN→所有source一件→解除／失効→停止を確認した。driver未対応を権限拡大・既存XDP置換で回避していない。
- [ ] 実client sourceと変換後tupleを確認し、設定の構文が通るだけで合格にしていない。
- [ ] HTTPS証明書・期待body・管理通信・宣言した対象外通信のbaselineを記録した。
- [ ] 初期化失敗・所有記録不明時に未知fileを保全して止まり、固定recoveryの手順を使えることを確認した。新binaryで未実施の異常系は未実施と記録した。

## C. 承認した公開canary一件

各操作は[操作手順](OPERATIONS.md)に従い、return code、instance／revision、期限、counter差、canaryと正常clientの結果をprivateに記録します。payload・cookie・秘密鍵を収集しません。

- [ ] 基礎防御だけ→空BAN装着で、正常HTTPS・証明書・期待body・管理・対象外通信が継続した。
- [ ] previewでは件数・期限・revisionが変わらず、表示scope／IP／TTLと操作意図を照合した。
- [ ] 明示BANでcanaryだけの対象通信を遮断し、独立正常client・管理・対象外通信を維持した。
- [ ] 元の期限で自然失効した。再BANや時間制限延長で通していない。
- [ ] 新しい未失効BANを確認してから明示解除し、通信復帰を確認した。失効を解除成功と取り違えていない。
- [ ] bypassを試す場合は新しい有効BANから実施し、control_revisionを使用した。offによる再遮断の影響も確認した。
- [ ] 承認した通常restartで新instance・BAN0・正常通信を確認し、旧commandを再投入していない。
- [ ] 公開siteには256件満杯、SIGSTOP／SIGKILL、所有記録破損の障害注入をしていない。それらは隔離証拠として分けた。
- [ ] 終了後、unit停止・非enabled、所有XDP非装着、試験process／待受なし、正常HTTPS・管理通信復帰を確認した。
- [ ] before／afterの保全確認、失敗・未実施事項、必要な証拠を残した。

## 中止条件

想定外のsource／tuple、管理source未確定、未知XDP、canary以外の通信劣化、管理喪失、Map結果不明、所有不明、容量異常、解除・停止後に通信が戻らない場合は中止します。
commandの自動再送、期限延長、権限拡大、未知file削除、firewall全flush、host rebootで成功扱いにしません。[復旧手順](RECOVERY.md)で対象だけを止めます。

## 二つの完了判定

**試験導入の技術的完了:** A・Bを満たし、承認したCのcaseが成功し、中止条件なし。最後に停止・非enabled、通信復帰、保全を確認できた。
未実施caseがあるなら範囲を明記し、そのcaseを含む完了とは報告しません。

**継続採用:** 上記とは別に、基礎防御だけでは足りない具体的な対処場面があり、同じ条件で効果と運用負担を比較した。
正常通信の遅延／成功率、必要な処理量、CPU／RSS等、検知から操作確認までの人手時間・誤操作を、比較前に決めた受入基準で判断します。
任意のRPSや改善率を後付けせず、既存の期限付きfirewall等で足りるなら不採用も選びます。canaryの遮断成功だけで実攻撃の検知能力や製品価値を証明しません。

最新の[実XDP記録](../review/2026-10-25-LOCAL-BAN-OPERATOR-KERNEL-ACCEPTANCE.md)と、[採用価値の判断案](../review/2026-10-25-PILOT-SHORTEST-PATH-AND-VALUE-GATES.md)を参照してください。これらも公開siteの合格票ではありません。
