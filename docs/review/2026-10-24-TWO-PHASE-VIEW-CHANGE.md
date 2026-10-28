# 二段階合意・証拠付き交代・旧履歴の保全

> WAL更新（2026-10-24）: 認証付きcheckpoint・状態転送・合意による失効記録整理を実装。通常daemonはcontext 5 / WAL v8です。旧WALを自動変換しません。[契約・上限・検証記録](2026-10-24-BOUNDED-WAL-CHECKPOINTS.md)が以前の保存方式の記述に優先します。

最終ゴールは小規模サイトでの安全な試験導入。今回の対象は、競合する未確定prefixがある状態から、確定履歴・投票・期限を守ってleader交代と合意を再開すること。prepare/commitの分離と旧WAL非自動移行はユーザー選択済み。本書は以前の単一QC方式の記述に優先する。

## 実装契約

通常daemonと `--init-state` は新方式を使う。旧方式は既存の反例・シミュレーションを比較できるよう残すが、通常daemonが旧方式へfallbackする経路はない。

| 項目 | 新方式 |
| --- | --- |
| 署名コンテキスト | consensus protocol 4 + cluster ID + config epoch + configuration digest |
| 構成の正規化・TLS | 既存の固定構成digest・ノード別TLSを使用。構成schemaと合意protocolの版は別 |
| phase署名 | `VEIL_PHASE_V1`、prepare/commit、view、index、累積hashを結合 |
| 交代報告の署名 | `VEIL_VIEW_REPORT_V1`、view、candidate、voter、prepared/committed証拠を結合 |
| RPC | `VEIL_RPC_V4` + protocol 4のcontext。新しい `TwoPhase` tagを末尾へ追加 |
| WAL | v7。phase別投票履歴、prepare証明、各heightのcommit証明、署名済み交代報告、採用new-viewを保存 |
| 未確定の幅 | 1 height。先行提案がある場合、直接BAN受付はbusyを返す。witnessed要求は元の期限で待機 |
| 上限 | 固定member 1..7、serialized command最大1024 bytes、wire最大65536 bytes |

定足数は `q = floor(2N/3) + 1`。安全性の前提は固定された異なる署名鍵、故障数 `f < N/3`、暗号の健全性、成功したWAL保存の持続性である。4台では1台故障を対象とし、7台・2故障の全配送順序を試験済みとは主張しない。進行には誠実な定足数の相互通信と保存が成功し、最終的に誠実なleaderが選ばれることが必要。

## 通常経路

1. leaderは署名付きの定足数交代報告を保持する。提案にはこのnew-viewと、entry・親prefix hashを添える。
2. followerは候補者binding、報告の署名・人数・phase・view、引継ぎ値、確定済み親prefix、有限TTLなどのpolicyを検証する。
3. `(Prepare, view, index) -> hash` をWALへ保存してからprepare票を返す。同slotの別hashは拒否する。この時点で適用しない。
4. leaderがprepare票の定足数証明を配布する。受信側は証明と対象entryを保存し、`(Commit, view, index) -> hash` と一緒にWALを確定してからcommit票を返す。
5. commit票の定足数証明だけが確定を許可する。受信側は順序と親hashを検証し、証明・ログ・commit位置を保存してから適用する。prepare証明をcommitとして扱わない。
6. backend障害は未適用状態を維持し、daemon tickや同じcommit証明の受信で再試行する。再投票や期限の更新は行わない。WAL障害は停止をラッチし、応答・投票・適用を継続しない。

entryのbirth term、内容、絶対期限、累積hashは再提案でも変更しない。票を出すviewは別フィールドであり、古いentryを新viewで安全に合意し直せる。phaseが違う署名と旧単一QC署名は相互利用できない。

## 交代経路と選択規則

既存のpre-vote定足数によるterm承認、fresh challenge応答、要求単位の検閲timeoutを維持する。新方式では、ログの長さ・最終termによる候補者排除は行わない。遅れた候補者は証拠を取得して追いつける。

- 新viewへの移行をWALへ保存した時点から、旧viewのprepare/commit票を出さない。報告にはcandidateを含め、同viewで別candidateへ報告しない。同じ報告の再送は最初に保存した内容を返す。
- 報告は、保持する最高のprepare証明と最新のcommit証明を含む。各証明のviewは報告先viewより小さくなければならない。
- 新leaderは定足数の報告を集め、受信側も同じ計算で `(height, 証明のview)` が最大の証明を選ぶ。同順位の競合証明は拒否する。各報告の署名と内包証明を検証する。
- 選択位置が未確定なら、そのentryを内容・期限・birth termを変えずに再提案する。すでに確定済みなら、そのprefixを保持した次の位置へ進む。報告に証明がない場合だけ、証明されていない未確定提案を置き換えられる。
- 新leaderが親履歴を持たない場合、1 entryずつcommit証明付きで取得する。未認証の「長いログ」を採用しない。旧投票は分岐を置き換えた後もWALに保持する。
- 後着のcommit証明はviewが古くても検証できる。ただし親prefixが連続し、既存の確定位置と同じhashであることが必要。
- ログ同期後のidle送信でもnew-viewを添える。commit証明の送信者をleaderとはみなさず、復帰ノードが検証済み交代証拠から現leaderを知り、fresh challengeを送れるようにする。

prepare/commitとview-changeの分離は[PBFT原論文 §4.2・§4.4](https://www.usenix.org/legacy/publications/library/proceedings/osdi99/full_papers/castro/castro_html/node4.html)を参照した。ただし原論文全体の実装ではない。こちらは未確定幅を1に制限し、次のheightへの票を出す前に親のcommit証明を要求する。原論文の複数sequenceにわたる証拠選択を、そのまま最高QCの選択へ置き換えたとは主張しない。

### 安全性を支える理由と限界

同phase・view・heightの二つの定足数には誠実な共通voterが必要になる。保存済みの投票を上書きしないので、そのvoterは競合する票を出せない。

commit証明が成立したなら、少なくとも `q-f` 台の誠実なcommit voterがprepare証明を保存済みである。後続new-viewの定足数はこの集合と交わる。報告前に旧viewへの署名を停止するため、報告時に見えなかった旧prepare証明から、あとで誠実な定足数のcommit票を集め直すこともできない。

選択のheightを先に比較できるのは、より高いheightの票を出す前に、その親prefixがcommit済みであることを検証するためである。同heightではより新しいviewの証明を優先し、そのview自身の選択規則が先の決定を引き継ぐ。これは設計上の帰納的な安全性説明であり、モデル検査器や証明支援系での一般証明ではない。有限の敵対的試験も一般証明とは区別する。

## 旧WAL・旧QCと運用

- 通常daemonはWAL v4/v5/v6をすべて拒否する。未解決prefixがある旧WALを保全するのはもちろん、ローカルで全件commit済みに見える旧WALも自動変換しない。停止した旧構成全体の移行証明は未実装。
- 旧WALを開いて削除・切捨て・新形式へ上書きしない。旧QCは旧方式の確定証拠であり、新prepareへ降格しない。
- 通常起動の既存WAL必須、署名鍵の排他ロック、一度限りの初期構築許可を維持する。同じ署名鍵で履歴を失ったまま新状態へ参加させない。
- 新規の隔離クラスタでは、専用の新しいidentityとcontextを用いて通常の `--init-key` → 固定構成/TLS発行 → `--init-state` を行う。既存の実運用identityを再初期化する移行手順ではない。
- 新旧binaryの混在、旧snapshotへの巻戻し、署名鍵とWALの別ホスト複製、動的membership変更はサポートしない。実運用への配布・移行は今回行わない。

直接 `--ban-ip` は単一leaderへの受付であり、busyなら受理されていない。検閲時の引継ぎが必要な要求には `--request-ban-ip` とrequest statusを使う。受付・確定・適用を区別し、期限が選挙や待機より短い要求を必ず執行する保証はしない。

## 検証と残課題

証拠ディレクトリ: `artifacts/two-phase-0mlci8az/`。開始前のsourceを保存し、既存の未コミット変更を保持した。macOSとpi-nodeのsource・build入力60ファイルのSHA-256一致を確認した。

実装に対応する試験は次を区別する。

- bare prepare票がA/Bに割れ、旧leaderが不在でも、通常の交代証拠で再開できること。
- prepareだけでは適用しないこと。commit票を返す前にprepare証明と投票履歴がWALへ残ること。
- 隠されたcommit QC、2回連続の交代、WAL復元、後着旧QCでも同じentry・期限を保持すること。
- 分岐の全8割当 × prepare証明の全8配送先集合 × 次の誠実leader 3候補 = 192通りで、成立したhidden commitに反する確定をしないこと。この有限fixtureは一height・一故障の範囲。
- 報告の省略・candidate改変・view改変・不正署名、誤った引継ぎ値、旧viewの遅延prepare、phase/context/旧QCの混用を拒否すること。
- 報告・prepare証拠/commit票の保存失敗時に署名応答や適用を出さないこと。旧WAL拒否で元bytesを保持すること。
- 遅れた候補者のcommit証明付き逐次catch-up、期限切れBANの不復活、適用再試行時の期限不変。
- 最大7 member・最大ID・最大command・最大署名集合でnew-viewを含むwireが上限内に収まること。超過commandはログや投票を変える前に拒否すること。
- 実ノード別TLS/QUIC、通常daemonの自動選挙、分岐解消、新しいBANの合意を確認すること。Mock版とLinux実XDP版を用意した。両方とも同プロセス内の4論理ノードで、旧leaderを停止して3ノードで復旧する。
- 別プロセスの実Linux/XDP試験と、上記の同プロセス・実QUIC試験を区別すること。

### 2026-10-24の実行記録

| 確認 | 結果・範囲 |
| --- | --- |
| macOS `cargo test --workspace` | 133 passed、0 failed、1 ignored。`mac-final-4.log` |
| pi-node Linux 7.0.0-38 x86_64 / Rust 1.98.1 | 133 passed、0 failed、6 ignored、3 filtered。`linux-idle-fix.log` |
| 新方式の専用回帰 | 13 passed。上記192通り、hidden commit、2回の交代、WAL保存失敗・復元、catch-up・期限保持を含む |
| 通信上限 | 最大構成の証拠入りenvelopeは42,233 bytes / 65,536 bytes。最大command超過の拒否も確認 |
| 分岐注入 + 実Linux XDP | 自動選挙 → ノード別TLS/QUIC → commit → 実パケットDROP → 署名期限後PASS。終了時XDPなし・ping成功 |
| 有限TTL・bypass・復元 | 専用Linux試験で、無期限BAN拒否、遅延catch-up、SIGSTOP中の期限失効、WAL復元、通常daemon/CLIのbypassを確認 |
| 起動・初期構築 | 通常binaryによる9確認がPASS。WAL欠損拒否、同一鍵での再初期化拒否、保持したWALによる復旧を含む |
| 4別プロセスの通常daemon | 7フェーズPASS。BAN/期限、follower受付、leader停止・再選出・WAL復帰、定足数喪失、期限後catch-up、TLS更新/失効。`linux-flow-result.json`、`linux-idle-fix.log` |
| 静的確認 | macOS/LinuxのClippy `--all-targets -- -D warnings`、fmt確認が成功 |

Linux全体回帰では専用実行が必要な6件をignoreし、既存のTUI/任意kernel系3件をfilterした。専用6件のうち、有限TTL、delayed catch-up、live TTL/bypass、今回のsplit recoveryを明示実行した。live TTLのchild fixtureもその試験から起動する。すべてのignored項目を網羅したという意味ではない。また旧単一QCエンジンの反例が再現できるテストも全体件数に含む。新方式の安全性確認は上記専用回帰と区別する。

途中の失敗もログを保持した。live TTL fixtureが旧WALを作って新daemonへ渡していたため、正しく起動拒否された。fixtureを新方式へ変更し、旧WAL拒否を弱めずに再検証した。4プロセス試験の初回はpi-nodeのkernel時刻誤差が既定の1秒を超え、新規BANを拒否した。ホストの時計・NTP・許容上限は変更せず、通常NTPの更新（08:46:54 JST）を確認して同一条件で再実行した。その後の4プロセス試験で、復帰ノードがログ同期後もleader不明のまま残る欠陥を発見した。`idle-leader-before.log` に再現失敗を保存し、idle送信にもnew-view証拠を付ける修正と専用回帰を追加した。修正後は4プロセス試験の7フェーズすべて成功した。

最終検証後に、独立してXDP attachmentなし・`veil-node`残存プロセスなし・既存隔離fixtureのping成功を確認した。既存interface/namespace、WAL・証拠は保持した。pi-node 1台上の論理ノード/別プロセス試験であり、4ホスト試験ではない。commit・push・公開・実サイトへの配布は行っていない。

小規模サイトへの投入可否は引き続き未判定。実サイトのsource IPと執行位置、運用者の復旧手順、ディスク上限・ログ整理・監視、複数ホスト間の障害、長時間運転を確認する必要がある。pi-nodeではNTP poll間隔34分8秒の運転中に1秒の時刻誤差予算を超え、一時的な新規BAN拒否を実測した。安全側停止は維持し、試験導入前に時刻同期と誤差予算の運用整合を確認する。WALの証拠は現状削除しないため増え続ける。通信・保存が成功する定足数を失った場合は停止する。新方式は署名・同期書込みと往復が増え、旧方式と同じスループットや遅延を約束しない。
