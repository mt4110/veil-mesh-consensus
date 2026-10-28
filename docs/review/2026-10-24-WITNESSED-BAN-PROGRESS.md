# 受付済みBANの進捗監視と検閲リーダーからの交代

最終ゴールは小規模サイトでの安全な試験導入。今回の対象は、freshなchallenge応答を返して選挙を抑制しながら、BAN要求をログに載せないリーダーからの交代である。要求の共有、個別の進捗時計、定足数の交代証明、元の期限を保持した引継ぎを追加した。任意のByzantine障害に対するBFT全体の完成を意味しない。

## 受付と利用方法

所有者限定のローカルソケットで、任意のmemberへ有限TTLの要求を預ける。

```sh
veil-node --control-socket /private/node.sock --request-ban-ip 192.0.2.72 --ttl-secs 60
veil-node --control-socket /private/node.sock --status-local --request-id RECEIPT_ID
```

`REQUESTED id=...` は受付ノードのWAL保存が成功したという意味であり、commit/applyの保証ではない。statusはqueued/logged/committed/applied/expired/unknownを区別し、元の絶対期限と該当ログindexを返す。実際の執行は従来の `--status-local --entry-index ... --status-ip ...` でbackend・適用エラー・ミラー・kernel counterも確認する。期限切れや時計不確かさで適用不能な要求は再発行しない。lost replyで新しい要求を盲目的に作らない。

従来の `--ban-ip` はローカルleaderへ直接proposeする経路として保持する。検閲耐性が必要な場合は新しい受付経路を使う。悪意あるノードだけに渡して他の誠実ノードへ届かなかった要求は検出できない。少なくとも1台の誠実な受付先と、そこから定足数へ配送できる通信が必要。

BANはexact IPv4の1件、TTLは1〜86400秒。発行時刻と期限は受付時に一度だけ確定する。`BanRequest` の署名はdomain tag、cluster ID、config epoch、構成digestを含む既存context、受付origin、canonical commandに束縛する。request IDはこのpayloadのSHA-256。保護IP、旧形式、TTL=0を受理しない。

originごとに未処理要求は1件。上限時は明示的なbusyエラーとし、既存の要求を追い出さない。終端済みreceiptは同originの新規要求で置き換わるため、永続的な検索台帳ではない。交代証明に含まれる要求も引継ぎ候補に含める。新しいleaderは既存ログに同じcommandがあれば重複追加せず、commit済みの同じcommandは解除後も再発行しない。

## 交代の条件と安全性の境界

全memberが要求を直接gossipする。各ノードは、認識するleaderとtermについて同じ未commit要求を5秒観測すると、request ID・term・leader・署名contextに束縛した進捗timeout署名を出す。freshなAck、NoOp、別のBANのcommitは、この時計をリセットしない。実際のleader/term変更と再起動では時計を開始し直す。5秒は現在の運用定数であり、WANの応答時間SLOや故意の検閲の証明ではない。

固定構成の異なる `floor(2N/3)+1` 台（4台なら3台）の署名から `ProgressCertificate` を作る。単独の高term、重複voter、署名偽造、contextやrequestの差し替えでは成立しない。gossip外側のtermは採用根拠にしない。

証明は旧termの終了を許可する。受信側は証明の `term+1` まで進みFollowerになる。その後は通常のランダム待機、pre-vote、排他的なRequestVoteで新しいleaderを選ぶ。典型例はterm 1の検閲からterm 2へ退避し、term 3で選挙する。証明自体はleaderの選出証明でもBANのQCでもない。

- 保存済み `voted_for` は同termで書き換えない。新しいtermの候補者投票もWAL保存後に応答する。
- 既存のログ新しさ検査、累積prefix hash、同 `(term,index)` の署名履歴、QC検証、適用前の永続化を通す。
- ログ競合を強制解消しない。新leaderが既存のロックで進めない場合、その制約を迂回しない。
- 交代対象は、証明の要求が未commitかつ有効な間、pre-vote支持、実投票、AppendEntriesの受理から除外する。証明を後から知っても同termの既存投票を取り消して投票し直さない。
- 除外証拠は最新term証明とは別にWALへ保存し、後続termや再起動で失わない。時計情報が取得不能な場合も、未commit要求の除外を解除しない。
- 元の要求期限を延長しない。遅延した交代証明は過去termへの追随に使えても、期限切れBANを作り直せない。同じ証明の再送で繰り返しtermを増やさない。

定足数の署名は、定足数が遅延を観測した証拠である。故意の検閲と過負荷・ネットワーク遅延を区別するものではない。2台しか通信できない4台構成では、交代やBAN commitを強行しない。

## 永続化・互換性・資源

WAL v6へ受付要求と除外証明を追加する。v4/v5の有効なWALは既存term、候補者投票、ログ、QC、署名履歴、期限とbypassを保持して読み込む。読込だけでは元ファイルを変更せず、次のcheckpointでv6へ保存する。新規要求の保存失敗時はreceiptを返さず、既存のpersistence停止ラッチに従う。再起動後に未成立の署名集合や経過時間は再利用せず、要求は元の期限で再共有する。

wireタグとTermCertificateのvariantを末尾追加した。既存のBAN command・QC署名形式、context v3、固定構成digestとTLS SANは維持する。旧binaryとの混在はサポートしない。全validatorを停止し、最新のWALと鍵を保全して全台更新する。v6保存後に旧binaryへ戻したり、古いWALへ巻き戻したりしない。

保持量は固定membershipの範囲に制限する。1originにつき1要求、1leaderにつき最新の除外証明、1要求・1voterにつき現termの1票。各gossipは1要求・1票・1証明とし、各集合を巡回する。既存の64KiB wire制限と800msのネットワーク往復上限を維持する。大量の署名検証やWAL全体の書換コストを含む負荷試験は今回の保証範囲外。

## 検証と残る条件

決定的試験ではfreshなAck、無関係なNoOp commit、少数票、偽造署名、高termヒント、除外後の再投票/Append拒否、WAL v5からの履歴保持、v6再起動、期限切れ、受付checkpoint障害を確認する。

実QUIC試験はノード別TLS・実daemon driver・4状態機械を同じプロセス内で動かす。故障leaderは要求を検閲し、freshなAckと無関係なcommitを続け、交代通知も無視して旧termのLeaderに居座る。残る3台で交代し、元の期限のBANを適用・失効することを確認する。この試験の執行backendはMockである。故障注入のフラグと分岐は `cfg(test)` のみで、通常binaryやCLIには存在しない。

別途、Linuxで通常binaryの4プロセスと実XDPを使い、Followerからの要求受付・commit・適用・期限失効を確認する。検閲試験を実XDPと同時に行った、複数の物理ホストで検証した、という意味ではない。

今回の完了は、この検閲条件からの安全条件を維持した交代と引継ぎである。競合する未commit prefix、任意のByzantine選挙妨害、帯域/CPU/ディスク飽和に対する完全なBFT進行性、実サイトの執行位置・source IP・監視・運用担当の受入れは残る。TTLが待ち時間より短い要求は失効し、交代しても新しいBANにはならない。短いTTLで必ず適用する保証はしない。

## 実行結果と証拠

| 検証 | 結果 |
| --- | --- |
| macOS workspace回帰 | 109成功、0失敗、1 ignored。最後の試験強化は対象8試験も再実行して成功 |
| pi-node / Linux x86_64 workspace回帰 | 109成功、0失敗、5 ignored、3 filtered。試験強化後にも全体を再実行 |
| 両OSのbuild / Clippy | 成功。Clippyは全target、`-D warnings` |
| Linux実QUIC検閲試験 | leader c→d、term 1→3。fresh Ackと無関係commitを観測。故障leaderはterm 1に居座り、誠実な3台で元の期限のBANを適用・失効 |
| Linux専用試験 | 有限TTL入口/WAL拒否、署名期限catch-up、SIGSTOP中のkernel失効、bypassと復旧が成功 |
| WAL起動black-box | 欠損拒否・初期構築の一回性・鍵lock・正しいWALからの復旧など9項目成功 |
| 通常daemon 4プロセス + 実XDP | Follower受付からのBAN適用・失効を追加した7段階成功。既存の再選出/WAL復帰、少数派catch-up、TLS更新/失効も成功 |
| 独立した終了後確認 | 当該daemon残留なし、XDP残留なし、隔離namespaceからping PASS |
| ソース照合・静的検査 | 実行ソース等56ファイルがローカル/Linuxで一致。cargo fmt、git diffチェック、Markdown 23ファイル成功 |

ignored/filteredを成功扱いしない。Linuxの環境依存/TUI試験3件は除外し、必要な専用XDP試験は別途明示実行した。実QUICの検閲試験と別プロセス・実XDPの試験範囲は上記のとおり分離している。複数物理ホスト、実サイトでの受入れは未検証。

途中、再起動試験のfixtureが新しいランダム鍵でWALを読もうとして署名拒否され、同一鍵で復元するfixtureへ修正した。安全条件を弱めていない。また既存の不正term証明のWAL試験をv6へ追随させ、単なる形式不一致ではなく、埋込み署名の不正で拒否されたことを確認するよう強化した。

証拠は `artifacts/censorship-w3re1h4b/` に保存。変更前source、今回だけのpatch、両OSの試験ログ、署名済み要求の進捗試験結果、`daemon-flow-result.json`、`wal-startup-result.json`、source hash照合、終了後確認を含む。Linux検証用コピーは `/home/maakie/veil-censor-DHOni00Z`。compiler/user試験用の短い一時領域は `/home/maakie/vcm-zqpsLj3i` を使い、既知の `/tmp` user quota問題を避けた。旧fixtureを削除していない。

実packet検査は既存の `veth-victim` / `ns-veil-rogue` のみ。物理NIC、host時計/NTP、既存firewall、本番設定は変更していない。node/CA秘密鍵を回収証拠へ含めず、commit/push・公開は行っていない。
