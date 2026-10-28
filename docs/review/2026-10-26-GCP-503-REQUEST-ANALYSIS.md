# GCP 503：保存要求・監査ログの読取り分析

> 後続: 保存した監査情報を使った[問い合わせ文を準備済み・本人確認待ち・未送信](2026-10-26-GCP-503-SUPPORT-DRAFT.md)。追加作成・新しいGCP API照会は行っていない。

2026-10-26 JST。Sol／High、分担なし。[zone b一回比較と撤去](2026-10-26-GCP-A-ZONE-B-COMPARISON.md)後、追加作成・課金・公開Bを行わずに調査する本人指示による。

**原因候補は絞れたが、根本原因は未確定。三回ともGCP側の監査ログでUNAVAILABLE（code 14）となり、記録された七権限は全てgrantedだった。boot disk・固定image・自動削除期限は意図どおり渡っていた。安全条件を外す修正は採用しない。**

最終ゴールは限定的な公開サイト試験導入の可否を判断できること。現在のマイルストーンはGCPの二VM準備で、まだ未達。今回は保存要求とエラーを照合し、原因候補・最小変更案・残作業を記録するところまで。製品コード、作成helper、GCP設定を修正する作業ではない。

## 調査範囲

- 既存の三つのVM作成要求・receipt・HTTP限定抽出を読んだ。原記録を上書きしない。
- ローカルGoogle Cloud SDK **561.0.0**のimage展開、boot disk選択、insert組立ての実装を確認した。更新・再インストールなし。
- 既に有効だったLogging APIで、該当時刻・`compute.instances.insert`だけの既存監査ログを取得。新API有効化、log bucket／sink／保存設定変更なし。
- 認証情報・metadata値・SSH公開鍵・個人emailを複製せず、status、選択したrequest項目、permission名とgranted、照合用insertId等だけをprivate artifactへ保存した。
- 既存Compute service agentへの`roles/compute.serviceAgent`の無条件bindingがあることを読取り確認。IAM付与なし。
- **15:30:22 JST時点の再確認でVM0・disk0。** 新設resource・追加作成要求は0。Pi・hp・router・Macネットワーク設定への操作なし。

原T0 **14:46:50 JST**、準備上限15:46:50、予備削除18:31:50、全体上限18:46:50、US$1を維持する。今回の読取り調査を理由に時計・予算をリセットしない。追加作成は今回の指示で禁止されており、残時間があっても行わない。実請求額の確定照合は未実施。

## 監査ログで新たに確認できた事実

| 要求 | UTC | zone | HTTP／監査status |
| --- | --- | --- | --- |
| 初回 | 05:49:21.644793 | us-central1-a | 503／14 |
| 一回の再試行 | 05:54:40.483892 | us-central1-a | 503／14 |
| 承認済みzone比較 | 06:15:06.026843 | us-central1-b | 503／14 |

messageの文言は`Internal error`だが、数値codeは13（INTERNAL）ではなく**14（UNAVAILABLE）**。[Googleのcode定義](https://raw.githubusercontent.com/googleapis/googleapis/master/google/rpc/code.proto)は14を503へ対応させる。これは障害の分類であり、内部の故障箇所や一過性であることの証明ではない。

三件とも、監査ログに出た次の七権限は全てgrantedだった。

`compute.instances.create`、`compute.disks.create`、`compute.subnetworks.use`、`compute.subnetworks.useExternalIp`、`compute.instances.setMetadata`、`compute.instances.setTags`、`compute.instances.setLabels`。

したがってMacからGoogleへ要求が届かない、CLI再認証がまだ失敗している、これら七権限で拒否された、という説明は観測と合わない。全ての内部権限・policyが正常と証明したわけではない。

### 実際に渡った構成

| 項目 | 監査requestの選択項目との照合 |
| --- | --- |
| machine | 各zoneのe2-smallの正規URI |
| boot disk | 一枚だけ。boot=true、autoDelete=true、20GiB、pd-standard |
| sourceImage | 固定した`ubuntu-2604-resolute-amd64-v20261021`の正規URI。URLの二重連結なし |
| scheduling | STANDARD、automaticRestart=false、instanceTerminationAction=DELETE |
| terminationTime | 三件とも`2026-10-26T09:31:50.468Z`。元の時刻がミリ秒精度へ丸められており、分・時単位でずれていない |
| Shielded | Secure Boot／vTPM／integrity monitoringの三つがtrue |

SDKの`image_utils.ImageExpander`は明示image URIをresourceとして解析し、`create/utils.py`はboot指定のcreate-diskを先頭に移す。監査requestも一枚なので、「URLをimageへ渡したため壊れた」「暗黙のboot diskが余分に作られた」という仮説を支持しない。

監査requestにはmetadata等が省略されている。**省略を「実際の要求に存在しなかった」と解釈しない。** 同様にserviceAccountsの省略だけを実VMの無権限証明としない。保存CLIはno-service-account／no-scopesを明示していたが、VM自体は成立していない。

## 原因候補の優先順位

| 候補 | 根拠と限界 | 判定 |
| --- | --- | --- |
| Compute内部のprovisioning処理・project側状態 | insertまで到達して14。二zoneで同一構成が失敗。サービス全体・region・projectのどこかは分からない | 先に調べる候補。故障箇所未特定 |
| 固定imageとmachine／scheduling等の組合せで内部処理が失敗 | 三要求が共通して持つ条件。構文や正規URIが合っていてもbackend固有不具合を排除できない | 候補として残す。imageや期限を犯人と断定しない |
| 同一regionの容量不足 | zone aとbは同じIowaで、独立したregion比較ではない | 排除できないが、典型的なRESOURCE_POOL_EXHAUSTED等の理由は出ていない |
| CLIの明白な要求変換ミス | boot一枚・image URI・期限・Shieldedは監査requestで整合 | 今回確認した項目では支持されない |
| callerの七権限不足・既知quota不足・Compute service agent role欠落 | 七権限granted、先行quotaに余裕、service agent roleあり | 典型的な説明の優先度は下がる。未観測の内部認可まで排除しない |
| XDP／WAL／TTL／BAN／guestの処理遅延 | VMが成立しておらず、製品を起動していない | 今回の503原因として扱わない |

[Computeのerror catalog](https://docs.cloud.google.com/compute/docs/reference/rest/v1/errors)では、quota超過の403と、resource pool不足の503などが別に定義される。今回の汎用messageだけで「quota増額」「別region」「大きいVM」を処方しない。

## 最小の修正案と不採用案

**現要求を直せば確実に作成できる、という修正は今回の証拠から出せない。製品コードとVMの安全設定を変更しないのが現時点の判断。**

次回の承認済み実行に向けた最小案は、作成内容を変えず、失敗後に今回と同じ限定監査項目を保存してrequest・時刻・insertIdで結び付けること。CLI stderrの`Internal error`だけで判断する不足を補う。既存API・既存ログの読取りで行い、権限追加・生のHTTP headerやcredential保存・自動再送を追加しない。**これは診断の改善案で、503を解消する修正ではない。今回はhelperへ実装していない。**

提供側への照会が可能なら、次の技術メモで三件の内部reasonを確認するのが先。問い合わせ送信や有料support契約は行っていない。

- project、UTC時刻、zone、監査insertIdは[private選択ログ](../../artifacts/gcp-503-analysis-odutkrnm/audit-status.json)へ固定。
- operationを観測できず、三件とも14、image／boot disk／期限の選択項目が整合、callerの記録された七権限がgrantedと伝える。
- Compute側が提供できる詳細reason／request相関情報、project provisioning状態、固定imageとe2-small＋期限付きDELETEの内部制約を確認する。秘密鍵・OAuth token・metadata値を送らない。

不採用または現時点で実施しない案は次のとおり。

- 自動削除期限やShieldedを外す：原因の証拠がなく、安全条件を弱める。[期限付きDELETEは通常VMでも公式に定義](https://docs.cloud.google.com/compute/docs/instances/limit-vm-runtime)されている。
- `--create-disk`を別表記へ変えるだけ：実requestが既に正規化されており、有効な修正とする根拠なし。
- Ubuntu26.04を一律非対応とする：公式の[OS対応表](https://docs.cloud.google.com/compute/docs/images/os-details)にも掲載され、固定imageはREADY。特定imageの不具合までは否定できない。
- roleを広く追加、default service accountを付与、quota増額：根拠なし。権限や費用を増やさない。
- 別image・machine・regionを続けて作成：一変数比較の候補にはなるが、現在の証拠で一つを優先できず、今回の追加作成禁止にも反する。
- 無課金dry-runとしてinsertする：公式の`instances.insert`にはvalidateOnlyパラメータを確認できない。作成要求は送らない。requestIdは重複抑止用で、dry-runではない。[REST定義](https://docs.cloud.google.com/compute/docs/reference/rest/v1/instances/insert)

## 残りと終了条件（今後の短い報告形式）

**残り四工程。環境が正常で承認・DNS操作が揃えば、実試験から撤去まで2〜4時間を工程上の目安とするが、未実測であり、503解消待ちを含まない。現在、全体の完了時刻は出せない。今回の四時間枠を延長する見積りではない。**

| 残り | 状態 | 何ができれば終了か |
| --- | --- | --- |
| 1. VM作成障害を解消し二VMを準備 | 読取り分析完了、根本原因未確定 | 別途認められた実行で二VMが成立し、期限・所有・SSH・費用・撤去経路を確認 |
| 2. GCP隔離実XDP | 未着手 | 同一成果物で正常通信・遮断・解除・失効・256件／257件・競合拒否・restart消失・停止復帰を確認 |
| 3. 公開Bの限定受入れ | 未承認・未着手 | 実値で承認後、公開HTTPSと独立正常client、期限前解除・自然失効・restart等、[六条件](../local-ban/DELIVERY_GATE.md)を証拠で判定 |
| 4. 撤去・証拠・導入判断 | 今回作成分の撤去は済み。公開試験後の最終撤去は未実施 | 新設物の撤去、既存資源との照合、費用確認、文書を揃え、限定試験導入の可否を明示 |

全工程の合格で今回の限定試験導入判定を終える。失敗時の撤去・報告は作業終了であり、公開受入れ合格へ読み替えない。次の一手は作成を増やすことではなく、今回の相関情報に基づいて内部reasonを得ること。今後は「残り／次の一手／終了条件」を短く併記する。

## 証拠と今回の完了条件

private artifact: `artifacts/gcp-503-analysis-odutkrnm/`。

- [監査query](../../artifacts/gcp-503-analysis-odutkrnm/audit-query.json)・[三件のstatus](../../artifacts/gcp-503-analysis-odutkrnm/audit-status.json)・[request／権限の選択項目](../../artifacts/gcp-503-analysis-odutkrnm/audit-request-selected.json)。
- [service agent roleの有無だけ](../../artifacts/gcp-503-analysis-odutkrnm/service-agent-role-selected.json)、[分析・三要求の整合検査・元の上限](../../artifacts/gcp-503-analysis-odutkrnm/analysis-summary.json)。
- [現在VM](../../artifacts/gcp-503-analysis-odutkrnm/current-instances.json)・[disk](../../artifacts/gcp-503-analysis-odutkrnm/current-disks.json)。九区分の撤去後照合は[前回の報告](2026-10-26-GCP-A-ZONE-B-COMPARISON.md)の時点のものと区別する。

今回の読取り分析・候補整理・最小案・履歴記録は完了。503の修復、VM／guestの回帰、公開試験は未達。[試験履歴](../local-ban/TEST_HISTORY.md)へ戻る。
