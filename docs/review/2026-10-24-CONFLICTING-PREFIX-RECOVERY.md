# 競合prefix復旧：反例と合意方式の判断

> 2026-10-24 更新: prepare/commit分離と署名付きnew-viewを通常daemonへ実装。consensus context 4 / WAL v7を使用し、旧WALは自動移行せず保全して拒否する。後述の旧方式・旧WAL互換性より[新方式の契約](2026-10-24-TWO-PHASE-VIEW-CHANGE.md)を優先する。

最終ゴールは小規模サイトでの安全な試験導入。今回のゴールは競合する未コミットprefixがあっても、投票・QC・WALの保護を維持してleader交代と合意を再開すること。有効なcommit QCを新leaderが取得できる場合の限定復旧は実装した。QCが取得できない競合からの一般的な復旧は未完了であり、下記の反例と合意方式・旧履歴移行の判断を残す。

## 現行実装で再現した停止

4台 a/b/c/d、定足数3台。aは旧leaderである。

1. aがterm 1、index 1の異なるコマンドA/Bを署名して送る。b/dはA、cはBを保持して各prefixへ投票する。どの受信ノードのcommit_indexも0。
2. aを以後の通信から外す。b/c/dの通常pre-voteとRequestVoteでbをterm 2のleaderへ選出できる。
3. bが現在termのNoOpを追加して通常の複製を8往復しても、b/dの2票しか一致しない。cのprefixはBのままで、commit_indexは0に留まる。

変更前のAppendEntriesはprev indexとtermで照合するため、同じterm/indexの異なるprefixを初期照合では識別できない。cがBの末尾へNoOpを追加して応答しても、累積hashはbの想定と異なり、その票は拒否される。先頭から送り直すだけでも、同termの競合と保存済み投票の保護によって置換は拒否される。安全側の拒否が進行を妨げている。

変更後は親hash不一致を即座に拒否して再送位置を戻すが、QCのない分岐は置換しないため停止が残る。再現試験は `reproduces_conflicting_uncommitted_prefix_stall_after_successful_election`。この試験の成功は「停止を再現できた」という意味であり、復旧できたという意味ではない。

## ロック緩和だけでは修正できない理由

同じ分岐でaは、自分とb/dの署名からAの有効なQCを作り、他ノードには配布しないことができる。b/c/dのローカルcommit_indexとcommit_qcsは依然として空である。WALを正しく復元しても、保存済みの個別投票だけでは、その署名を集めたQCが他所に存在しないことを証明できない。

もし「新termなので未commit部分は捨ててよい」と変更すると、b/c/dがterm 2の同じindexでBへ署名し、BのQCも作れる。各 `(term,index)` の二重署名を拒否し、古い投票履歴を残したままでも、term 1のAとterm 2のBの両QCが暗号的に検証を通る。

`hidden_qc_survives_wal_and_naive_higher_term_rebase_can_create_conflicting_qcs` は、この変更を仮定した反例をtest内だけで構成する。現在の製品コードがその上書きを許すと主張するものではなく、既存の拒否条件を外してはいけない根拠である。製品の安全機構を無効化して試験を通していない。

「受信した報告で一番長いログ」「最大termのログ」「報告にQCがない」「3台が同意した」「期限切れBANだからログを捨てる」のいずれも、それ単独では旧QCを無効にする根拠にしない。BANの執行期限と合意履歴の確定性は別である。

## 実装済み：既知のcommit QCによる限定復旧

- AppendEntriesに必須の `prev_log_hash` を追加し、既存prefixとの一致をログ変更・投票の前に検証する。term/indexだけ一致する別prefixの上へ新しい票を出さない。
- 受信entry列から一時的なcandidate prefixを作り、QCのcontext・定足数署名・term/index・累積hashを検証する。競合する全位置がQCで覆われ、受信commit位置まで確定できる場合だけ未commit部分を置換する。検証失敗は元のログ・投票を保持する。確定済みprefixとの競合はQCがあっても拒否する。
- 旧 `(term,index)` の投票は削除も上書きもしない。QCで確定した末尾が旧投票と異なる場合、RPC自体の署名付き受領応答を返すが、QC投票署名は空にする。leaderは自身も確定済みの一致prefixに限って、この応答を複製位置の更新に使う。空署名を未commit位置の票として数えない。
- 復旧で切り捨てたsuffixの票も保持する。leader側のローカル `propose` にも既存投票との衝突検査を追加し、同じslotへの異なる再署名を拒否する。新しい選挙・termでの延長が必要。
- WAL保存・署名・適用の順序は維持する。保存失敗時は停止をラッチし、受領応答や投票を送らず、kernelへ適用しない。

この経路は、取得済みの確定証拠に追随する処理である。QCを他ノードから探索する新プロトコルや、どの候補が選ばれてもQC保有者がleaderになる保証は追加していない。QC未取得、QCで覆われない既存suffixとの競合、wire上限のため証拠と対象範囲を一緒に届けられないケースでは、停止が残り得る。

### 互換性

通信の署名domainを `VEIL_RPC_V3` から `VEIL_RPC_V4` へ変更した。旧schemaの署名を新schemaとして解釈しない。全memberを停止して同じ新バイナリへ更新する必要があり、rolling upgradeや新旧混在をサポートしない。この変更を実運用ノードへ配布・適用したという意味ではない。

QC・term証拠・要求証拠の署名domain、cluster/config context v3、WAL v6は変更しない。既存WAL v4/v5/v6の読込みと投票保護を維持する。限定復旧にprepare/commit方式への自動移行は含まれない。以下の新方式の移行条件と混同しない。

## 一般的な復旧方式の選択

| 方針 | できること | 制約・互換性 |
| --- | --- | --- |
| 推奨：prepare/commitを分けた合意方式と検証可能なnew-view | 単なる提案・個別投票と、他viewでも保持すべき決定を区別し、競合からの一般的な復旧を設計できる | 新しい署名phase、WAL、wire、交代証拠が必要。旧QCを新prepareとして読み替えない。未解決prefixを含む旧WALは保全し、自動移行しない |
| 限定：既存の有効なcommit QCが覆うprefixだけを復旧 | 取得できたQCが認証する決定へ追随する。既存QCの意味を維持できる | QCが取得できず個別投票が割れた場合は停止を維持する。今回の一般的な合意再開のゴール全体は満たさない |

どちらも既存WAL・署名鍵の削除、空状態からの再加入、少数票での確定、ロック条件の無根拠な緩和を復旧手順にしない。旧プロトコルの安全性をそのまま維持しつつ、すべての未解決状態を自動変換できるとは約束しない。

## 推奨案の実装契約（採用判断待ち）

参照するのはPBFTのprepare/commitとview-changeの分離である。原論文では、prepareによる同view内の一致だけでなく、commit前に証拠を複数の正しいreplicaへ残し、後続viewでその証拠を引き継ぐ。新leaderの選択結果も各replicaが検証する。[Castro/Liskov, §4.2・§4.4・§4.5](https://www.usenix.org/legacy/publications/library/proceedings/osdi99/full_papers/castro/castro_html/node4.html)

以下はこのリポジトリへの適用条件であり、原論文のコードを実装済みという意味ではない。

### 正常経路

- 提案の識別子にcluster/config context、phase、view、height、親prefix hash、command digestを含める。ネットワーク上でも親hashの不一致を明示拒否する。
- proposal/prepareの定足数証拠だけではkernelへ適用しない。prepare証拠・対象データ・必要なlockをWALへ保存してからcommit票を送る。
- commit phaseの独立した定足数証拠を検証・永続化した後でcommit_indexを進める。期限付きcommandの内容・期限は変更しない。
- leaderの主張だけでphaseやprefixを進めない。新viewの承認と候補者・提案者の権限を結び付け、同viewに複数の候補者証明が来ても排他的投票を保持する。
- 少数派、disk/fsync障害、clock不確かさ、backend失敗は既存の失敗時動作を保持する。

### 交代経路

- view-changeへ移る前に旧viewへの署名停止を永続化する。報告後に旧viewのcommit票を出せる状態を残さない。
- view-change報告は検証可能なprepare/commit証拠と必要なデータを含める。「自分の最大term/index」という自己申告を選択根拠にしない。
- 新leaderは定足数の報告から採用対象を計算し、選択を検証できるnew-view証明を配布する。最大値の選び方と、何もprepareされていない位置の扱いを仕様化する。
- 受信側が同じ選択計算を行い、保存済みの確定prefixを保持すること、証拠のphase・view・context・voterの一意性を検証する。
- 選択したcommandを新viewで再提案する場合も、元の絶対期限を維持し、旧viewでの投票履歴を削除しない。
- 遅延した旧QC、再送したnew-view、途中で再度交代した場合を含め、単調なviewと一意な決定を守る。

原論文はsequenceごとの証拠を扱う。一方、現行コードは累積prefixへのQCと複数entryのバッチを使う。この差を未検証のまま「最高QCを選べば安全」と置き換えない。最小実装では未確定の決定単位を限定する案も比較し、通常運用の吞み込み量より安全性の検証可能性を優先する。必要な署名段階が増えるため、現行の往復回数・書込みコスト・レイテンシを維持するとは主張しない。

### 旧WAL・旧QC

- 旧QCは旧方式のcommit証拠として保持し、新prepare QCへ降格・再解釈しない。
- 旧WALを読んで未commit suffixや未解決の署名履歴を検出した場合、元ファイルを保持して移行を拒否する。自動切捨ては行わない。
- 安定したcheckpointからの移行でも、単にローカルの `commit_index == log.len()` を確認するだけでは足りない。旧構成での署名停止、checkpoint外の投票履歴の扱い、旧/新contextへの結合を含む移行証拠が必要。
- 通常起動と移行操作を分離する。曖昧な履歴を解決できない場合は停止した状態を維持し、運用者がWALを削除して通過できる手順を用意しない。
- 新旧validatorの混在や旧snapshotへの巻き戻しをサポートしない。新規の隔離fixtureと、実在する旧WALの移行を別々に試験する。

### 完了条件と中止条件

1. 分岐A/B、隠れた旧証拠、prepareのみ成立、commit成立後の片側配送、2回以上の交代を決定的試験で区別できる。
2. どのviewでも同じ決定位置に競合するcommit証拠を作れないことを、敵対的な順序・重複・欠落とクラッシュ挿入で検証する。有限試験を一般的な証明とは呼ばない。
3. WAL保存の直前/直後の再起動、保存失敗、署名済み履歴の保持、新旧証拠の混用拒否を確認する。
4. 通常daemonで分岐を注入し、誠実な定足数だけで交代・合意・適用・元の期限での失効を再開する。QUIC/TLS試験と実XDP・別プロセス試験を区別する。
5. 既存の有限TTL、context binding、固定membership、鍵/WAL起動拒否、freshness、検閲対策を回帰する。
6. pi-nodeで同一sourceを確認し、隔離networkの後片付けと既存状態の保全を独立に確認する。

旧証拠を捨てる、投票上限を緩める、同じ署名鍵で履歴を失って再参加する必要が出たら、その案は中止する。一般的な復旧を実装したと報告できるのは、方式選択・移行契約・実装・上記試験を終えた後である。

## 検証記録

証拠は `artifacts/prefix-recovery-k5gpxmxe/`。開始前のsourceと差分を保全し、今回だけの変更を `task-only.patch` に分離した。既存の未コミット変更を保持し、commit/push/公開はしていない。

- macOS: `cargo test --workspace --locked` は119 passed / 0 failed / 1 ignored。内訳は既存109件、今回の回帰8件、未解決問題の反例2件。反例の成功を修正完了の証拠とは数えない。Clippy all-targets `-D warnings` 成功。
- 追加した回帰は、QC付き同term分岐の修復と旧票を保持する再起動、新termでの有限BAN再開、偽QC・範囲外suffix・確定済み競合の拒否、受領応答とQC票の区別、ローカル再提案の投票保護、保存失敗、旧RPC署名domain拒否、実QUIC/node TLS/通常daemonを通る復旧。
- 新しいdaemon試験は分岐とQCの片側配送を注入し、通常の署名付きpre-vote/RequestVote経路でbを選出してからdaemonを駆動する。選挙部分は直接メッセージ配送であり、任意の初期分岐から自動選挙が必ず適切なleaderを選ぶ試験ではない。修復・NoOp・ローカルBAN受付・合意・適用・元の期限での失効・WAL復元を確認する。TLS/QUICは実通信、パケットbackendはMockであり、分岐注入と実XDPを一体で試験したものではない。
- pi-node: Linux 7.0.0-38-generic / x86_64、Rust 1.98.1、source/vendor固定のoffline実行。57個の `.rs/.toml/.lock/.py/.c/.o` がローカルとSHA-256一致。全体回帰は119 passed / 0 failed / 5 ignored / 3 filtered。3 filteredは対話TUIと別実行するLinux専用経路であり、合格数へ加えていない。buildとClippy all-targets `-D warnings` 成功。
- Linux専用の実XDP検証を個別実行し、有限TTL ingress/旧WAL拒否、署名期限を超えたcatch-up、実TTL/緊急バイパス/復旧の3件が成功。新しい10件の試験も個別に再実行して成功した（うち反例2件は未解決を示す）。実node TLS/QUICの構成digest検証も成功。
- `wal-startup-check.py` の9項目成功。`daemon-flow-check.py` は実XDPと別4プロセスで7段階成功（自動選挙、受付拒否、BAN/失効、witness要求、leader停止/再選挙/WAL再起動、少数派/期限切れcatch-up、TLS更新/失効）。分岐注入はこの別プロセス試験には含まない。
- 事後の `bpftool net` に付着なし、`pgrep -ax veil-node` に残存プロセスなし。試験scriptも `cleanup_no_xdp=true` / `cleanup_ping=true` を記録。既存の `ns-veil-rogue` と `veth-victim` は保持した。物理NIC、本番サービス、host時計を変更していない。fixtureのWALと証拠は残す。
- `cargo fmt --all -- --check`、リポジトリの `scripts/lint-markdown.sh`（24ファイル）、`git diff --check` 成功。ネットワーク試験初回のfixture directory権限エラーは、試験専用directoryを最初から0700で作る修正後に再実行した。安全検査を無効化していない。

限定復旧の実装と一般的なBFT進行性を区別する。小規模サイトへの試験導入判定は引き続き保留。一般的な復旧は、prepare/commit・new-viewと旧WALの移行契約の選択、実装、反例検証が残る。
