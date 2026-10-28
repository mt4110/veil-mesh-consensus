# GCP準備：Compute Engine API有効化と作成前確認

2026-10-26 JST。対象は`veil-x-tests`のみ。最終ゴールは公開サイトでの限定試験導入、今回の完了範囲は承認済みAPIの有効化と作成前の読取り確認。[前段の事前確認](2026-10-26-GCP-API-PREFLIGHT.md)を保全し、本書を後続結果とする。

**Compute Engine APIのステータス「有効」を詳細画面で確認した。VM作成・公開・手動の権限追加・課金設定変更は未実施。公開サイトの受入れHOLDは維持する。**

## 承認と実行

本人から、画面のGoogle Cloud Platform利用規約への同意表示を踏まえたAPI有効化の承認を受けた。projectとサービス名`compute.googleapis.com`を照合し、「このAPIを有効にする」を一度だけ実行した。

元画面は進行中表示のまま残ったが、再クリックしなかった。別画面のservice account生成とVPC表示を確認後、元画面を再読込みするとVM一覧へ進んだ。さらに「有効なAPIとサービス」からCompute Engine API詳細を開き、サービス名とステータス「有効」を独立して確認した。規約表示の確認後、新しい請求先のリンクや追加の権限操作は求められなかった。

## 有効化後のリソース・権限表示

| 対象 | 観測と判断 |
| --- | --- |
| VM一覧 | 空で、「最初のVMインスタンスを作成」の案内。今回VMを作成していない |
| Compute Engine default service account | 事前一覧は空、事後は有効な既定account一件が表示され、キーは「ありません」。IAMではEditor（編集者）を確認。手動のaccount・鍵作成やrole付与は行っていない |
| Google管理identity | Google提供ロールを含む表示で、Google APIs Service AgentとCompute Engine Service Agentが見え、それぞれインスタンスグループマネージャ／Compute Engineのservice agent roleを確認。変更前の全内部identityを列挙できたとは扱わない |
| VPC | `default`、自動モード、MTU1460、subnet欄42、firewall rule欄4を表示。事前一覧は読取り失敗のため、全networkを今回生成と断定しない |
| default firewall | `default-allow-icmp`は0.0.0.0/0のICMP、`default-allow-ssh`は0.0.0.0/0のTCP22、`default-allow-rdp`は0.0.0.0/0のTCP3389、`default-allow-internal`は10.128.0.0/9のTCP／UDP全portとICMP。いずれも許可、priority65534。編集・削除していない |
| 請求先 | 既存の請求先accountの管理画面に`veil-x-tests`がリンク済みとして表示。新規リンク・支払方法変更なし。残高・credit・税・最終請求額の保証ではない |

既定VPCとEditor付きaccountをそのまま試験VMへ使う案にはしない。新規custom VPCと、VMにservice accountを付けない案を維持する。既定accountのrole変更やdefault rule削除は今回の承認範囲へ追加せず、生成物は保全した。

## 割当の読取り

| us-central1の表示名 | 上限 | 使用量 |
| --- | --- | --- |
| Compute Engine API / CPUs | 200 | 0 |
| Compute Engine API / In-use regional external IPv4 addresses | 8 | 0 |

quotaの増枠要求・編集はしていない。名前`E2 CPUs`および`CPUs (all regions)`での検索は該当なしだったが、これは上限ゼロを意味しない。地域の一般CPU行と外部IPv4行を確認した範囲であり、全quota・disk容量・zone在庫・作成成功を保証しない。実作成前の最終確認は残る。

## 作成前フォームでの候補確認と見積り

「インスタンスを作成」のフォームを開き、下記を画面上の未保存設定として選択した。最終の「作成」は押していない。OS／disk選択ダイアログの「選択」は候補をフォームへ戻す操作であり、disk作成の完了ではない。

| 項目 | 選択・表示 |
| --- | --- |
| machine | e2-small、2GBメモリ、共有core。専用2coreとは扱わない |
| region／zone | us-central1（Iowa）／指定なし。固定zoneと在庫は未確定 |
| OS | 通常版Ubuntu 26.04 LTS、x86/64 amd64、resolute、2026-10-21 buildと表示。Minimal／Pro／Armを選んでいない。完全なimage名・ID・hashは未取得 |
| disk | 新規標準永続disk、20GBの候補。画面のGB表示を記録し、実作成後の容量確認とは区別 |
| backup | 既定のsnapshot schedule `default-schedule-1`が選択されていたため、未保存フォームだけを「バックアップなし」へ変更。既存scheduleの削除や変更はしていない |
| 金額 | CPU＋メモリ $12.23／月、20GB標準disk $0.00、合計 $12.23／月、約$0.02／時と表示 |

diskの$0.00表示が無料枠等のどの条件によるかは確定していない。外部IPv4・通信・税等を含む総額ではないため、これを全費用ゼロやUS$1内の強制保証として使わない。[計画書の公式単価による費用例](../local-ban/GCP_TRIAL_PLAN.md#費用)と別に記録する。作成前に二台・最大四時間・停止後disk課金・撤去までを含めて見積りを確定する。

作成画面のdefault VPC、既定service account、backup、Ops Agent等を無確認で採用しない。今回候補の全項目を設定済みの完成した配備案とは扱わず、公開rule・管理経路・自動停止・通常clientの経路を含む具体案は次の段階とする。

## 携帯回線を使わない方針と終了

本人から携帯テザリングを今後利用しない指示を受けた。今回MacのWi-Fi変更や保持接続は実施していない。自宅WANの追加試験は保留し、再開指示がない限り実施しない。ルーター再ログインや携帯接続の維持を試験の前提にしない。

クラウドはAPI有効と上記生成物を保全した状態で残す。Pi・hp・router、Macのnetwork、製品コード・TTL・256件上限・再起動時BAN消失の契約に変更なし。作成前フォームは保存せず、今回のGCP作業タブは終了時に閉じる。

中止時の復旧案は、依存利用がないことを確認した上で当該APIのみ無効化すること。今回は有効化が成功したため、無効化は実行していない。依存APIの強制無効化や、自動生成account／network／ruleの削除は含めず、それらは残る。API無効化だけで有効化前の全resource状態に戻るとはしない。

次は[六つの受入れ条件](../local-ban/DELIVERY_GATE.md)を満たすため、携帯回線を使わないVM構成・公開範囲・見積り・復旧／撤去対象をレビュー可能にする。**今回のAPI承認をVM作成・課金・公開の承認に流用しない。**

[試験履歴](../local-ban/TEST_HISTORY.md)へ戻る。
