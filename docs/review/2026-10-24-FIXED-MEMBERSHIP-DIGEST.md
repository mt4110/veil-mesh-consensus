# 構成digestへの署名bindingと固定membership

2026-10-24（利用者側の記録日）。`7e9473c` を基点とする既存の未コミット修正へ追加。[前段のcluster ID・epoch binding](2026-10-23-CLUSTER-CONTEXT-BINDING.md)からの差分を記録する。

最終ゴールは小規模サイトでの実践投入。現在のマイルストーンは構成・署名・復旧の境界を固めること。今回の完了条件は、同じcluster ID・epochでも構成が違えばRPC/QC/WALを拒否し、固定した投票者集合を検証し、回帰試験とpi-nodeで確認すること。今回のタスクは完了した。本番投入の判定は保留する。

## 修正した問題

以前はcluster ID・epochが同じなら、member集合・鍵・接続先・保護方針の違いを署名から検出できなかった。また、通常daemonは公開鍵ディレクトリを定期的に読み直して信頼する鍵を変更し、peer IDをアドレスから仮生成していた。これでは実際の投票者集合と署名対象の構成が分離してしまう。

今回、検証済みの `FixedConfiguration` から署名context、peer ID/接続先、公開鍵集合、定足数の母数、保護IPを一度だけ設定する。未登録の`.pub`ファイルを自動採用する経路は通常daemonから外した。

## digestの契約

構成はSHA-256でdigest化し、`SigningContext.config_digest`（32 bytes）へ格納する。既存依存のring、serde、bincodeを利用し、依存追加はない。

canonical payloadは次の順序のbincode tupleである。文字列は長さ付き、memberはnode ID順、IP集合は数値順でserializeする。

| 順序 | 内容 |
| --- | --- |
| 1 | schema識別子 `VEIL_FIXED_CONFIG_V1` |
| 2〜4 | signature protocol version、cluster ID、config epoch |
| 5 | 固定profile `all-voters:floor(2N/3)+1` |
| 6 | node ID順のmember一覧。各memberはID、正規化したIP socket address、32-byte Ed25519公開鍵 |
| 7 | 実効保護IPv4集合。既定の保護IP、全IPv4 member endpoint、追加設定の和集合 |
| 8〜10 | 最大TTL（86,400秒）、時計誤差上限ms、boot clockの最大遅延率前提（1,000ppm） |

入力順序の入替え、公開鍵hexの大小文字、同じ保護IPの重複はdigestを変えない。IPv4-mapped IPv6 endpointはIPv4へ正規化する。同じendpointを別表記で重複登録した場合は拒否する。意味が同じ構成を一致させ、実効構成の差を比較するための処理である。

RPC、QC vote、Timeout voteの署名payloadにこのcontextを含める。署名domainは `VEIL_RPC_V3`、`VEIL_QC_VOTE_V3`、`VEIL_TC_VOTE_V3`、protocol versionは3。外側のdigestを変更しても署名は成立せず、新しい外側署名で古いQCを包み直しても内側の検証で拒否する。ログhashそのものは変更しない。

構成digestは、ノードが起動時に採用した信頼済み構成に対するbindingである。Byzantineノードが内部でどの設定を実行しているかを遠隔証明する機構ではない。

## membershipの検証と固定

全memberを投票者とする固定profile。observerや重み付き投票は導入していない。1〜64 memberという上限は入力・資源の制限であり、64台の性能・BFT進行性を実証した数値ではない。

- node IDは1〜128文字のASCII英数字・`_`・`-`。空ID、path文字、重複IDを拒否する。
- 公開鍵は64桁hexから32 bytesへ変換。形式不正、全ゼロ、重複鍵を拒否する。同じ鍵を複数IDへ登録して定足数を水増しできない。
- endpointはIPアドレスと非ゼロport。DNS名、未指定IP、multicast、IPv4 broadcast、scope/flow情報付きIPv6、正規化後の重複endpointを拒否する。接続可能性やDNS解決を構成検証に混ぜない。
- 自ノードがmember一覧に存在し、読み込んだ秘密鍵から得た公開鍵がそのmemberと一致することを要求する。ID重複を先に拒否するため、自ノードは正確に1件となる。
- peer一覧はmember一覧から自ノードだけを除いて導出する。公開鍵レジストリは自ノードを含む同じmember集合。定足数の母数を別のCLI一覧から作らない。
- インストール後は鍵レジストリの差替え、identity変更、署名context変更、固定構成の再設定を拒否する。digestだけを直接設定する経路も拒否する。

公開鍵の事前検査は形式・重複と自ノード秘密鍵との整合の検査である。各remote memberの秘密鍵所持は、実際の署名検証で確認する。すべてのremote鍵が運用可能であることやendpointの到達可能性を起動時に証明したとは扱わない。

従来の状態機械fixture/TUIはdigestゼロのsimulation contextを使える。通常daemonは必ず検証済み固定構成から非ゼロdigestを導出するため、simulation署名を同じ構成として扱わない。

## 通常daemonの起動契約

`--cluster-id`、`--config-epoch`に加え、自ノードを含む全memberを `--member ID@IP:PORT=HEX_PUBLIC_KEY` で繰り返し指定する。全ノードへ同じ一覧を配る。追加保護IPは `--protected-ip IPv4`、時計誤差上限は既存の `--max-clock-error-ms`（既定1,000）。これらも全ノードで一致させる。

`--peer` と公開鍵ディレクトリからの自動登録は廃止した。`--listen` のport/address familyは自ノードmemberと一致させ、bind IPはそのmemberのIPまたは同じfamilyのwildcardを許可する。通常daemonでport 0は許可しない。ローカルのbind IPがwildcardか否か、ファイルパス、XDP interface名はcluster共通データではないためdigestに含めない。

鍵生成は、daemon起動から明示的な初期化へ分けた。次は新しい検証環境用の操作形であり、既存validatorの鍵の作り直しや移行手順ではない。

```sh
veil-node --init-key --node-id node-a --key-file /path/to/node-a.pk8
```

出力はnode IDと公開鍵hexだけ。新規秘密鍵は従来どおり0600で保存し、既存ファイルは権限等を検査して読み込む。通常daemonでは `load_existing_file` のみを使い、鍵が欠けた場合に生成して稼働を続けない。

例えば4台なら、次のように同じ4件を全台へ配り、各台でnode ID・秘密鍵パス・listenを選ぶ。`PUB_A`等は初期化で得た公開鍵hexへ置き換える。実サイト向けの完成した配備手順ではない。

```text
veil-node --node-id node-a --cluster-id site-example --config-epoch 1 \
  --key-file /path/to/node-a.pk8 --cluster-dir /path/to/cluster-state \
  --listen 0.0.0.0:5000 \
  --member node-a@10.0.0.11:5000=PUB_A \
  --member node-b@10.0.0.12:5000=PUB_B \
  --member node-c@10.0.0.13:5000=PUB_C \
  --member node-d@10.0.0.14:5000=PUB_D \
  --protected-ip 192.0.2.10
```

member指定はnode IDと鍵・接続先を結び付ける起動入力であり、署名済み構成配布や専用manifestファイル形式は今回追加していない。配布元と起動引数の保護は運用側の信頼前提である。

## WALと変更の扱い

WALはv4へ変更した。保存されたcontextにdigestを含め、復元先の構成と一致しなければログ・投票・復元ルールを変更せず拒否する。member順序等だけが違う同値構成なら同じdigestとなり復元できる。

**旧WAL v1/v2/v3と旧署名形式は非互換。自動変換しない。** 旧v3へ後付けで構成digestを推測して付けることはしない。前回の実WAL v3のコピーを新daemonで読み込み、拒否と原本・コピー双方のhash不変を確認した。

既存WALを削除して同じ鍵で空stateから再参加したり、version/contextだけを書き換えたりしない。構成変更後は、同じepochでも異なるdigestの既存WALを拒否する。これは誤混在の停止であり、安全な構成移行を実装したことではない。epoch更新、旧構成の停止・fencing、履歴を維持した移行・rollbackは別途必要である。

WALのchecksumは破損検出であり、ローカル管理者による悪意あるファイル書換えを防ぐ署名ではない。WAL・鍵・構成一式の古いbackupへの巻戻し検知も今回の保証外。

## 検証結果

| 検証 | 結果 |
| --- | --- |
| macOS `cargo test --workspace --locked --offline` | 72 passed / 0 failed / 1 ignored |
| pi-node Linux回帰 | 72 passed / 0 failed / 4 ignored / 3 filtered |
| macOS / Linux Clippy `--workspace --all-targets -- -D warnings` | 両方成功 |
| 整形・差分空白・Markdown lint | 最終出力を証拠に保全 |
| 固定構成digestの実mTLS QUIC試験 | 成功。同じcluster ID・epoch・鍵・CAでも異なるdigestのRPC/QCを拒否。現在構成のQCは確定 |
| XDP期限catch-up | 固定構成で成功。期限済みBANは再武装せず、新しいBANはDROP後に期限でPASS |
| TTL・バイパス・WAL復旧・通常daemon | 固定構成・WAL v4で成功。未登録`.pub`ファイルの拒否も確認 |
| 実daemonの不正起動9ケース | すべて拒否しstate directory未作成。既存秘密鍵は不変、欠落鍵は生成されない |
| 前回の実WAL v3 | 読込み拒否。原本と検証コピーを保全 |

新たな8件の回帰試験は、canonical化、不正member集合、自ノードの整合と構成固定、同epochでの構成差によるRPC/QC/TC/WAL拒否、重複投票と未登録鍵による定足数水増し拒否、実QUIC、起動引数、欠落鍵の非生成を覆う。既存の署名・WAL・期限・wireサイズ回帰も維持した。

構成差はmemberの追加・削除・ID変更・鍵変更・endpoint変更・保護IP変更・時計誤差上限変更を試験した。拒否時のstate/WAL不変、同値構成での復元、現構成の署名による成功を対照条件に含める。単に通信が失敗しただけでは合格としていない。

途中で実QUICの対照試験に失敗した際は、試験用leaderがすでに配信済みのnext indexを使っていたことを修正し、新規受信ノードへ先頭から配信する状態へ揃えた。受信側のprefix/QC検査は弱めていない。Linux側の新しいClippyの固定長配列読取り指摘も、警告抑制ではなく実装で解消した。

## pi-nodeの範囲と証拠

Linux 7.0.0-38-generic / x86_64、Rust 1.98.1。検証コピーは `/tmp/veil-membership-fSqcPef0`。元checkoutを編集せず、loopback mTLS QUICと既存の隔離veth/namespaceを使用した。ホスト時計・NTP・物理NIC・既存firewallは変更しない。

通常daemonの試験では起動前に未登録の `peers/outsider.pub` を置き、同じCA・現contextで署名した未登録者の投票を実QUICで送った。応答なしで拒否し、WAL bytes不変を確認した。その後、既存のバイパスCLI・実パケット試験も成功した。

TTL catch-up試験の観測値はkernel報告誤差367,000,000ns、対照段階のPASS=3・DROP=1。SIGSTOP段階ではPASS=1・DROP=1。終了後にXDP attachmentなしとping PASSを確認し、既存fixtureは保持した。複数ホスト・広域ネットワーク・本番負荷の実証ではない。

ローカル証拠は `artifacts/fixed-membership-qfkw759z/`（git管理外）。開始時snapshot/hash、`linux-check.txt`（Clippy指摘で停止した初回）、`linux-final.txt`（修正後完走）、`startup-check.json`、最終source照合、今回だけの差分を保存する。生成秘密鍵やCA秘密鍵は証拠archiveに収集しない。変更は未コミットで、push・配備・既存ファイルの削除は行っていない。

## 残る投入条件

digestの対象はここで定義した合意member・接続先・執行方針・時計設定である。TLS証明書・CA trust bundle・ノード固有TLS identity、ローカル実行pathやOS設定すべてをhashする機能ではない。現行の共有CA運用を本番用PKIへ置き換える課題は残る。IPv6通信の遮断や保護CIDR全体の検証も未実装。

安全な構成/epoch移行、BFTのview-changeと進行性、通常daemonの提案入力・自動駆動・観測、site固有の運用受入れは未完了。構成混在を拒否できることと、分断後に必ず合意が進むことは別の性質として扱う。

## 後続実装

[通常daemonの自動選挙・BAN受付・合意/適用・期限失効](2026-10-24-DAEMON-OPERATION-FLOW.md)を接続し、4プロセスの隔離Linux受入試験を完了した。上記の通常daemon未完了という記載は本記録作成時点の状態である。BFT view-change・本番PKI・構成移行・site受入れは引き続き未完了。

## 後続のWAL初期構築変更

通常起動は正常な既存WALを必須とし、初回の空state作成は `--init-key` が発行する一度限りの許可と `--init-state` へ分離した。新規クラスタの起動例にはこの操作が追加で必要。既存鍵で欠損WALを再作成する操作ではない。[起動契約・互換性・復旧の境界](2026-10-24-WAL-BOOTSTRAP-RECOVERY.md)を優先する。
