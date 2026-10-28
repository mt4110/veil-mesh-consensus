# 遮断範囲・独立承認・個別解除の実装契約

2026-10-24。通常daemonは署名context 6 / WAL v9を使用する。この文書は旧資料の直接BAN受付、全通信遮断、WAL互換性の記述に優先する。試験導入先は未決定で、本番投入を承認するものではない。

## 権限と侵害時の境界

合意ノードの署名は提案者の身元を示すが、対象IPを遮断してよい根拠にはならない。通常daemonでは、固定membershipの鍵と異なる運用者Ed25519公開鍵を全ノードへ設定する。運用者の秘密鍵は別の管理端末に保管し、ノードへ配布しない。署名器はノード鍵、TLS秘密鍵、WALを必要としない。

BANと個別解除は、運用者が対象送信元IPv4、操作、TTL、絶対期限、受付期限、次の承認revisionを署名したファイルだけを受け付ける。署名はcluster ID、epoch、構成digestを含み、構成digestには運用者公開鍵と許可したサービス範囲を含める。raw BAN、raw解除、LSM操作は通常daemonの合意経路で拒否する。保護IPは運用者署名があってもBANできない。

1ノードが侵害されても、運用者鍵のない攻撃者は健全ノードに新しいBANを承認させられない。4固定member・定足数3・侵害1台という既存合意の前提を維持する。侵害ホスト自身のrootによるカーネル変更、運用者鍵の漏洩、正規運用者の誤判断は防げない。これは複数運用者の共同承認ではなく、単一の独立運用者による承認である。署名は対象IPが悪性であることの証明ではない。

## 遮断範囲

`--scope IPv4/tcp/PORT` または `--scope IPv4/udp/PORT` を1〜16個指定する。宛先IPv4・プロトコル・宛先ポートが一致する通信だけに送信元IPのBANを適用する。1件の承認は、そのクラスタで宣言した全scopeを対象とする。scopeをBANごとに選ぶ機能やCIDR指定はない。範囲は全memberで共通で、変更すると構成digestとTLS識別名も変わる。

対象は **plain Ethernet上の、断片化されていないIPv4 TCP/UDP**。IPv6、VLAN、IPv4フラグメント、その他のプロトコルは対象外としてPASSする。フラグメントを使う通信はBANを回避できるため、全面的な送信元IP遮断とは呼ばない。これらの経路が利用可能なサイトで本機構だけを強制的なアクセス制御にしてはならない。全ポートへ遮断を広げて対処する実装にはしない。

同じ宛先IP・ポートを共有するHTTPホスト名、URL、利用者は区別できない。CDN・リバースプロキシの後ろでは、見える送信元が利用者ではなくプロキシになり得る。最初の試験対象は、送信元IPと対象サービスが明確な自己管理Linuxの隔離サービスに限定する。共有WebホスティングやL7でしか利用者を識別できない構成は、このXDP方式の導入対象ではない。

`--status-local` は `policy_authority`、`scope_coverage`、`service_scope_N` を返す。`policy_mirror=Drop` は範囲内の送信元ルールの状態であり、全通信が遮断されたという実測結果ではない。

## 解除・再送・失効

承認revisionはクラスタ全体で直列化し、現在の確定revisionに1を加えた値だけを認める。BANと解除は同じ順序に並ぶ。解除は対象IPのルールを削除する操作であり、全体バイパスを有効にしない。別IPのBANは残る。

最後のルールを解除した後もrevisionはWALと定足数認証付きcheckpointに残る。状態転送・再起動・古い承認の再送によって解除済みBANが復活しない。同じ次revisionで複数の操作を署名した場合、先に確定した操作だけが有効となる。残る操作は現状を確認して明示的に再承認する。revisionの自動書換え・再署名は行わない。

新規投票は検証済み時計の誤差区間全体が承認の受付期間内にある場合に限る。受付期間は最大300秒で、BANの絶対期限を超えない。署名CLIは2秒の時計余裕を発行時刻から差し引くため、TTLは2秒より長くする。通信・合意時間もTTLに含み、TTL=60なら実際の残時間は60秒未満になる。署名端末も正しい時計を必要とする。

期限内にprepare証明が成立した操作は、受付期限後も正当な証明またはnew-view選択に従って完了できる。BAN期限は延長されず、既に失効していればDROPしない。prepare未成立の期限切れ提案は同じtermの投票を取り消さず、リーダー交代で処理する。隠れたprepare証明が後から現れた場合はそれを尊重する。

個別解除は合意を要する。定足数喪失中の即時復旧は既存の所有者限定 `--bypass-local on` を使う。この操作はそのノードの全scopeを一時的にPASSさせるもので、個別解除とは異なる。解除の受付成功は合意・実適用の成功ではない。requestとentryの状態、hash、全ノードの適用状況、エラー、実通信を確認する。

## 操作例

以下は新規試験クラスタ用。`PUBLIC_CONFIG` はBash配列で、固定全member、cluster ID、epoch、保護IP、時計上限、運用者公開鍵、scopeを含む。各ノードの公開設定はオフライン署名端末へ渡せる。秘密を含めない。既存鍵でWALを作り直して移行してはならない。

```bash
# 管理端末だけで、一度だけ新規作成。出力は運用者公開鍵。
veil-node --init-approver --approval-key-file ./operator.pk8

# 実環境の公開値へ置換する。memberは全員を列挙する。
PUBLIC_CONFIG=(
  --cluster-id site-trial --config-epoch 1
  --node-id a --listen 192.0.2.11:5000
  --member "a@192.0.2.11:5000=$A_PUBLIC_KEY"
  --member "b@192.0.2.12:5000=$B_PUBLIC_KEY"
  --member "c@192.0.2.13:5000=$C_PUBLIC_KEY"
  --member "d@192.0.2.14:5000=$D_PUBLIC_KEY"
  --approval-public-key "$OPERATOR_PUBLIC_KEY"
  --scope 192.0.2.10/tcp/443
)

# daemon側の所有者ソケットで現在のauthorization_revisionを取得。
veil-node --control-socket /run/veil/control.sock --status-local

# 管理端末でRを現在の確定revisionに設定し、承認ファイルを新規作成。
veil-node "${PUBLIC_CONFIG[@]}" --approval-key-file ./operator.pk8 \
  --expected-policy-revision "$R" --approve-ban-ip 198.51.100.77 \
  --ttl-secs 60 --approval-output ./ban.approval

# 承認ファイルだけをノードへ渡し、任意memberに受付を依頼。
veil-node --control-socket /run/veil/control.sock --submit-approval ./ban.approval
veil-node --control-socket /run/veil/control.sock --status-local --request-id "$REQUEST_ID"

# 新しい確定revisionを確認して個別解除を承認。
veil-node "${PUBLIC_CONFIG[@]}" --approval-key-file ./operator.pk8 \
  --expected-policy-revision "$CURRENT_REVISION" --approve-unban-ip 198.51.100.77 \
  --approval-output ./unban.approval
veil-node --control-socket /run/veil/control.sock --submit-approval ./unban.approval
```

通常の初期WAL構築、TLS名生成、TLS検査、daemon起動にも同じ公開profileを渡す。daemonへ `--approval-key-file` を渡す運用は禁止する。承認ファイルは上限1024 bytesで厳密復号し、既存出力を上書きしない。オンラインWeb承認UI、権限委譲、運用者鍵ローテーション、無停止構成移行は未実装。

## 互換性・安全性

prepare/commit証明、投票前WAL永続化、new-view選択、認証付きcheckpoint、容量上限を維持する。承認revisionをsnapshotへ追加したため、通常daemonはcontext 6 / WAL v9。旧v8を含むWALを自動変換しない。旧証明・旧構成の署名を新形式へ読み替えない。ノード署名鍵と履歴を一体で保全し、同じ鍵での再初期化は行わない。

旧隔離ICMP fixture用のbridgeはテストビルドだけに残る。製品daemonはscopeなしのXDP attachを拒否し、scope map ABIのない旧BPFオブジェクトも拒否する。旧fixtureの通常daemon起動検証は「未承認profileを拒否してWALを保全」へ変更し、新しい通常daemonのBAN・解除・バイパスはPythonのblack-box試験へ移す。

## 検証記録

ソース・生成BPFを含む72ファイルのhashをローカルとpi-nodeで照合し、差分なし。非公開のローカル証跡は `artifacts/approval-scope-usntdxc8/`、Linux側は `/home/maakie/vap-5PfdEb/` に保全した。秘密鍵を報告へ出力していない。

- macOS全体回帰: 155 passed / 0 failed / 1 ignored。その後追加した個別解除の独立性を含む承認回帰8件も通過。fmt、clippy、Markdown lint通過。
- pi-node（Linux x86_64）直列全体回帰: 155 passed / 0 failed / 6 ignored / 3 filtered。対話・専用実カーネル試験を通常suiteから分離。最終ソースの承認回帰8件とclippyも通過。
- 最終ソースのroot実カーネル試験: TTL/SIGSTOP・バイパス・WAL復旧、署名済み共通期限の遅延catch-up、競合prefixからの自動交代・合意再開の3件が通過。旧未承認profileの通常daemon拒否とWAL保全も確認。
- 通常daemon 4プロセスのblack-box試験: 自動選挙、raw要求拒否、運用者でも保護IPをBAN不可、承認BANの確定・適用・SIGSTOP中の失効、follower受付、リーダー停止とWAL再起動、定足数喪失時の未確定表示、未prepare承認の期限切れ後のnew-view復旧、TLS更新、個別解除、改ざん署名と旧BANの拒否が通過。
- 宣言したTCP/UDPポートへ送った実パケットはDROP、同じ送信元の別ポート・別宛先・ICMPはPASS。解除後の対象TCP/UDPはPASS。ホストの既存UFWが試験用ポートを拒否したため、アプリケーションecho試験はbaselineで停止した。その失敗記録を残し、UFWを変更せず、隔離vethのXDP通過後・UFW到達前のAF_PACKET観測へ変更した。HTTP疎通やTCP接続確立の試験ではない。
- 高さ32・64でcheckpointを作り、1台停止中の状態整理、復帰時の認証付き状態転送、XDP所有ノードのWAL再起動を実施。古いBANは復活せず、その後の承認revision 57まで全4ノードが一致した。fixtureのcheckpoint WALは6,242〜6,314 bytes。実負荷の性能保証ではない。
- WALの欠損・空・破損時の起動拒否、同じ鍵での再初期化拒否、正確な保全WALからの復旧というblack-box 9条件を再確認。TLS provisioningの前提試験も通過。
- 別コマンドで、試験後にXDP attachmentなし、veil-nodeプロセスなし、既存interface/namespace保持、隔離ping成功を確認。ホストのUFW・NTP設定は変更していない。

**追跡先:** [並行回帰でのタイムアウト調査](2026-10-24-PARALLEL-CENSORSHIP-RECOVERY.md)に、その後の原因調査・修正・成功と再発の記録を追加した。以下は本段階の記録として保全する。

**未解決の検証結果:** Linuxで2スレッド並行の全体回帰を2回実施した際、既存の検閲リーダー交代テストがタイムアウトした。単独実行と直列全体回帰では通過したが、原因は確定していない。制限時間・期待値・安全条件は緩めていない。並行負荷での進行性を合格扱いにせず、試験導入前に追跡する。診断途中の単独実行では永続化拒否も発生し、その記録も保全した。指定した試験用TMPDIRでの最終検証は通過している。

実カーネルで検証したBPFはCから生成した同梱オブジェクト。Rust版eBPFにも同じscope照合を反映したが、この作業ではRust版のBPFターゲットビルド・ロードを検証していない。実サイト、多ホスト障害、長時間負荷、IPv6/VLAN/断片化の保護、運用者鍵の復旧・ローテーション、無停止移行は未検証または未実装。今回の完了は限定scopeでの承認・個別解除の実装と隔離検証であり、小規模サイトへの試験導入全体の完了ではない。
