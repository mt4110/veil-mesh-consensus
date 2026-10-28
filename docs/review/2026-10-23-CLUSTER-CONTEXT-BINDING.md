# cluster ID・config epochへの署名binding

2026-10-23。`7e9473c` を基点とした既存の未コミット修正へ追加。前段は[署名付き共通期限](2026-10-23-SIGNED-DEADLINE-CATCHUP.md)。この記録は、本番投入やBFT全体の完成を意味しない。

最終ゴールは小規模サイトでの実践投入。現在のマイルストーンは署名と復旧の境界を固めること。今回の完了条件は、同じ鍵を使っていても別クラスタ・別構成世代のRPC/QCを受理せず、タイムアウト投票とWALにも境界を適用し、回帰試験とpi-nodeで確認すること。今回のタスクは完了した。

## 修正前の問題と採用した範囲

後続修正（2026-10-24）: [固定membership・構成digest](2026-10-24-FIXED-MEMBERSHIP-DIGEST.md)を追加した。以下はその前段の記録であり、現行は署名protocol v3 / WAL v4。起動には全member一覧と既存署名鍵が必要。

従来のRPC署名はRPC本体だけ、QC署名はterm・index・entry hash、タイムアウト署名はterm・high QCのterm/indexが対象だった。鍵とノードIDを別環境で再利用した場合、署名から所属クラスタ・構成世代を識別できなかった。TLSで止まることに依存せず、合意署名そのものを分離する。

`SigningContext` を導入した。

| フィールド | 契約 |
| --- | --- |
| `protocol_version` | 2。旧署名形式へのfallbackはない |
| `cluster_id` | 1〜128文字のASCII英数字・`.`・`_`・`-`。大小文字を区別し、空白等を自動補正しない |
| `config_epoch` | 正のu64。Raft termやポリシーのlog indexとは独立した構成世代 |

運用者はクラスタごとに一意のIDを割り当て、構成変更には別epochを割り当てる。同じID・同じepochを別の構成へ使い回す操作は、本機能では識別できない。構成内容のdigest、固定membership manifest、鍵・TLS identity・保護方針まで含めた完全な構成照合は残課題である。

これは固定構成の署名namespaceを分離する変更であり、稼働中のmembership変更やepoch移行プロトコルではない。旧構成を停止・fencingせずに新構成を並行稼働してよい、という仕様ではない。

## 署名payloadと検証順序

既存のbincode 1.3.3を継続し、次の順序のtupleをserializeする。文字列は長さ付きで、単純連結による境界の曖昧さを避ける。依存追加はない。

| 署名用途 | tupleの要素順序 |
| --- | --- |
| RPC | `"VEIL_RPC_V2"`, context, signer ID, RPC本体 |
| QC vote | `"VEIL_QC_VOTE_V2"`, context, term, index, entry hash |
| Timeout vote | `"VEIL_TC_VOTE_V2"`, context, term, high QC term, high QC index |

RPC本体のenumはmessage kindを含む。署名付きRPC envelopeにもcontextを持たせ、受信側が起動時に設定したcontextと一致しなければ、term変更・投票・ログ変更・適用より前に拒否する。一致した場合も、ローカルの期待contextからpayloadを再構築してEd25519署名を検証する。envelopeの識別情報だけを書き換えても署名は成立しない。

QC/TCの署名検証にもローカルの期待contextを必ず渡す。QC/TCの構造体自体にcontextを重複保存する必要はなく、外側RPCから受け取った未検証contextを検証条件として採用しない。現在のcontextで外側だけを署名し直しても、別contextの内側QC・タイムアウト投票を流用できない。

ログentry hashの計算方式は変更しない。同じログbytesから同じhashが得られても、署名のnamespaceが異なれば別の証明になる。署名付き期限の値はこれまでどおりentry hashを経由してQCへ束縛される。

QC生成、QC検証、AppendEntriesResponse内のQC投票、TimeoutVote生成・検証、TC検証の各経路を更新した。署名contextはstate内で非公開に保持し、term・ログ・投票・永続化開始後の設定変更を拒否する。

署名envelope増加分は64KiBのwireサイズ計算へ含める。最大長128文字のcluster IDと最大u64 epoch、32×128ルールの分割転送で上限を維持した。wire enumの大きなvariantはBoxで保持し、Clippy指摘を解消した。Boxはメモリ上の保持方法であり、追加のwire fieldではない。

## daemon起動と互換性

通常daemonでは `--cluster-id` と `--config-epoch` の両方を必須にした。未指定・epoch 0・不正IDは起動エラーで、デフォルトnamespaceへフォールバックしない。既存のTUI・状態機械fixtureは明示的なsimulation contextを使い、ローカルバイパスCLIは既存daemonのソケット操作なのでこの指定を必要としない。

起動構文は次の形になる。実サイトの構成は未決定であり、これは配置・鍵配布・安全な移行の手順全体ではない。

```text
veil-node --node-id node-a --cluster-id site-example --config-epoch 1 \
  --cluster-dir /path/to/cluster-state [既存のlisten・peer・WAL等の引数]
```

全ノードでcontextを一致させる。古い起動スクリプトは新しい引数なしでは停止する。既存のpeer ID/公開鍵読込みの制約や、共有CAの本番配布問題を今回解決したとは扱わない。

WALをv3へ変更し、contextを保存した。復元先のnode IDとcontextを照合し、不一致なら復元state・カーネルルール・投票を変更せずエラーにする。ファイルの自動書換えはしない。WALのchecksumは破損検査であって、ローカル管理者の悪意ある書換えを防ぐ署名ではない。

**WAL v1/v2は読込み拒否する。** v2には署名namespaceの根拠がないため、新しいID/epochを推測して付け足さない。前回の「v2を読める」という契約はこの変更以前のものである。wire署名も非互換であり、旧バイナリとの混在稼働・rolling upgradeは非対応。

既存WALを削除して同じvalidator鍵で空stateから再開したり、WALヘッダのversion/contextだけを書き換えたりしない。過去の投票を消さずに移行するには、旧構成の停止・fencingと履歴の検証を含む別の移行手順が必要。その手順・自動変換は今回実装していない。新しいepochのCLI値だけを変えて既存WALを再利用することも拒否する。旧WAL・鍵・構成は保全する。

## 回帰検証

| 検証 | 結果 |
| --- | --- |
| macOS `cargo test --workspace --locked --offline` | 64 passed / 0 failed / 1 ignored |
| pi-node Linux回帰 | 64 passed / 0 failed / 4 ignored / 3 filtered |
| macOS / Linux Clippy `--workspace --all-targets -- -D warnings` | 両方成功 |
| `cargo fmt --all -- --check` / `git diff --check` / Markdown lint | 最終確認の出力を証拠に保全 |
| 実mTLS QUICによるbinding試験 | macOS・Linuxで成功。Linuxでは専用出力も記録 |
| Linux期限catch-up専用試験 | 成功。期限切れはPASS、新しい決定はDROP後に期限でPASS |
| Linux TTL・バイパス・復旧専用試験 | 成功。SIGSTOP、WAL、通常daemon/CLIを含む |

Linux通常回帰の除外3件は対話TUI・既存専用Linux試験。専用試験の実行は通常回帰の合格数と区別する。今回追加した7件は次を確認する。

- contextの形式検証、合意開始後・永続化開始後の変更拒否。
- 全5種類のRPCで別クラスタ・別epoch・envelope書換えを拒否し、WAL相当stateが不変。未知protocol versionも拒否。
- 同じ鍵・同じログhashでも、外側を正しく署名し直した旧QC・TC・TimeoutVoteを拒否。現在contextの新しいQCは合意できる。
- 別contextのWAL、旧versionのWALを無変更で拒否。同一contextのログ・投票・QCは復元できる。
- 最大context長を含めた64KiB分割転送。
- 同じCAとEd25519鍵を共有した実mTLS QUICでも、旧RPC/QCを拒否してWAL不変。現在contextのQCは確定・適用できる。
- 従来のcontextなし署名と用途の異なる署名の拒否。

途中では、新しいテストのborrow修正と、term 0のNACKを通常の受信入口へ流していた試験手順を修正した。NACKは署名を直接検証し、term 0拒否の安全条件は維持した。Clippyはwire variantのサイズ増加を指摘し、抑制せずBox化した。

## pi-nodeで確認した境界

Linux 7.0.0-38-generic / x86_64、Rust 1.98.1。検証コピー `/tmp/veil-binding-zucYGSyL` を使用し、元のcheckoutは変更しない。

通信試験はloopback上の実mTLS QUIC。同じCA・同じノードID・同じEd25519鍵・同じログ内容を使い、`site-a / epoch 7` から `site-b / epoch 7` と `site-a / epoch 8` への再利用を試みた。両方で旧RPCを拒否し、現在contextで包み直した旧QCにも拒否応答を返した。拒否前後のWAL bytesは一致。対照として現在contextで新規署名したQCを配るとcommit/applyが成立した。

通常daemonの起動チェックではcontext未指定、epoch未指定、epoch 0をすべて終了コード1で拒否し、state directoryが作られないことを確認した。さらに前回の実機試験が生成した実際のWAL v2をコピーして起動し、version拒否と原本・コピー双方のhash不変を確認した。生成した鍵の内容は出力・証拠archiveへ収集していない。

既存の隔離veth/namespace上の実XDP回帰も再実行した。期限切れcatch-up段階ではDROP増加なし、新しいTTL付き決定ではPASS=3・DROP=1を観測。SIGSTOP中のTTL、期限切れWAL復旧、緊急バイパス、設定復元・明示off、WAL v3と新CLI引数による通常daemon起動も通った。試験終了後にXDP attachmentなし・ping PASSを確認した。ホスト時計・NTP・物理NIC・既存firewallは変更していない。

最初の専用試験スクリプトは実行ファイルの選択を誤り、通常バイナリが `--exact` を拒否して停止した。その試行は専用試験の成功と数えていない。CargoのJSON artifact情報からtest executableを取得するよう修正し、専用試験を完走した。

## 証拠と残課題

ローカル証拠は `artifacts/context-binding-ipy11r2k/`（git管理外）。開始時snapshot・差分・hash、`linux-check.txt`（回帰/build/Clippy成功と後段スクリプト停止）、`linux-live.txt`（専用試験完走）、`startup-check.json`、最終source manifest・ローカルと実機のsource照合・今回だけの差分を保全した。秘密鍵は収集しない。既存ファイルの削除・commit・push・本番変更は行っていない。

残る重要事項は、構成digestと固定membership検証、epochを安全に移行する手順、BFTのview-change/進行性、通常daemonの入力・同期・観測、site固有の保護対象と運用受入れ。ID/epochとWALをまとめて過去へ巻き戻す管理操作は、このbindingだけでは検知できない。単一ホストの検証を、多ホスト運用・本番保護・構成移行の証明へ拡大しない。
