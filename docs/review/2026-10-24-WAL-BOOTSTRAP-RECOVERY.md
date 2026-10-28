# WAL初期構築と欠損時の起動拒否

最終ゴールは小規模サイトで安全に試験導入できる状態。今回の範囲は、初回の空state作成と通常の復旧を分離し、WALを失った既存署名鍵で投票を再開しないこと。BFT進行性や一般的なバックアップ巻き戻り検出は別の課題である。

## 起動契約

| 操作 | 必須条件 | 動作 |
| --- | --- | --- |
| `--init-key` | 新しい鍵pathとbootstrap record path | Ed25519鍵を排他的に作成し、node ID・公開鍵に束縛した一度限りの初期化許可を隣接する `.bootstrap` ファイルへ保存する。既存鍵は再発行しない |
| `--init-state` | 新規鍵の未使用許可、固定構成、明示的な鍵/WAL path | 許可を消費・fsyncした後、新規WALを排他的に予約し、空の初期stateを永続化する。daemon・QUIC・control socket・XDPは開始しない |
| 通常起動 | 既存署名鍵、正常な既存WAL、固定構成、ノードTLS bundle | 鍵をlockし、既存WALを検証・復元してから投票を開始する。欠損/空WALはXDP attachより前に拒否する |

通常起動はWALがなければ空stateを生成しない。存在するWALは従来の版・checksum・node ID・署名context検査を受ける。構成digestには合意公開鍵も含む。破損WALを空stateで置き換えるfallbackはない。

WALの安定lockに加え、署名鍵ファイルもdaemonの全期間lockする。同じ鍵ファイルまたはhard linkを使ってWALだけを変えた同時起動も拒否する。ローカルのOS file lockであり、別ファイルへコピーした秘密鍵や別ホストまでの排他ではない。

## 新しい試験クラスタの初期構築

1. 各ノードで未存在のpathを指定して `--init-key` を実行する。公開鍵出力を固定member一覧に組み込む。
2. 全員が同じ構成を使うことを確認する。各ノードで同じ構成・自分の鍵を指定し `--init-state` を一度だけ実行する。
3. [ノードTLS運用](2026-10-24-NODE-TLS-OPERATIONS.md)に従ってCSR・発行・bundle配置・事前検査を行う。TLS準備とstate初期構築の順序は交換可能だが、どちらも通常起動前に完了させる。
4. 通常起動では初期化済みのWAL pathをそのまま指定する。control socketは未使用のpathを用意する。

以下は全member指定が必要な操作形である。`$PUB_A` 等は各 `--init-key` の公開鍵出力、`$KEY_A` と `$WAL_A` は管理者が選ぶnode-aのpath。既存稼働クラスタの鍵再作成・epoch移行手順ではない。

```sh
veil-node --init-key --node-id node-a --key-file "$KEY_A"

veil-node --init-state --node-id node-a --listen 10.0.0.11:5000 \
  --cluster-id trial-site --config-epoch 1 \
  --key-file "$KEY_A" --wal-file "$WAL_A" \
  --member "node-a@10.0.0.11:5000=$PUB_A" \
  --member "node-b@10.0.0.12:5000=$PUB_B" \
  --member "node-c@10.0.0.13:5000=$PUB_C" \
  --member "node-d@10.0.0.14:5000=$PUB_D"
```

初期化の成功はクラスタ合意やBAN適用の成功ではない。空WALのdurabilityを確認する操作であり、選挙は通常daemonを起動して初めて始まる。

## 失敗・復旧・既存環境

- 既存pathへの `--init-state` は拒否し、WALを上書きしない。
- 使用済み許可では、WALが欠損していても、別pathを指定しても再初期化を拒否する。`--init-key` を既存鍵へ再実行して許可を作り直すこともできない。
- 許可消費後のWAL作成/fsync失敗、途中クラッシュ、部分的な許可recordは自動修復しない。残存ファイルを保全し、その鍵で投票を再開しない。無理に許可を未使用へ戻さない。
- この変更以前の鍵は未使用の証拠を持たないため、新しい空WALを作れない。正常な既存WALと鍵の組は、`.bootstrap` がなくても通常復旧できる。既存WALの形式変更はない。
- WAL欠損時は、そのノードを停止したままにする。別nodeのWAL、古いsnapshot、空WALで代用しない。鍵・IDの再生成だけで同じ構成へ戻さない。
- 試験で確認する復旧は、別名で保全した**同一の最新WALを元pathへ戻す**ケースである。一般のバックアップが投票履歴を網羅しているかを自動判定する機能ではない。

## 保証の境界

署名鍵・bootstrap record・WALを管理者専用の保存領域で扱う。秘密鍵を別pathへコピーして再利用しない。bootstrap recordは新規鍵が初回stateを作るためのローカルguardであり、秘密鍵とは別の外部信頼anchorではない。

管理者/rootによるrecord改変、初期化前の鍵と許可の複製、ディスク全体の古いsnapshot復元は検出できない。既存WALが古い有効なWALに置き換わった場合も、checksumだけでは検出できない。これらには外部の単調な履歴・fencing等の追加設計が必要であり、今回完了とはしない。

実行中にWAL pathを失った場合、既存WAL用storeは次のpersistで再作成せずエラーになる。内容に変化のないcheckpointは従来どおり書込みを省略するため、欠損を常時監視する機能ではない。再起動時の欠損拒否とは区別する。

## 検証

単体試験は許可の一度限りの消費・既存/legacy鍵への再発行拒否・別identityの許可・権限・部分record・鍵lock/hard link/symlink・欠損/空WAL・初期化途中の失敗を覆う。

`scripts/wal-startup-check.py` は通常binaryで、初期構築、選挙/NoOp確定、同じ鍵で別WALの同時起動拒否、WAL退避後の起動拒否と再初期化拒否、破損/空WAL拒否、元の最新WALからの復帰を確認する。fixtureは削除せず、秘密鍵を含むdirectoryを0700で保全する。Unix socketはOSのpath長制限を避けるため専用の短い `/tmp/veil-wal-control-*` 配下に作る。

4プロセスの `scripts/daemon-flow-check.py` も、明示的に各ノードの初期WALを作ってから通常起動する手順へ変更した。既存のTLS・TTL・WAL復旧・BAN・再選挙の安全条件は変更していない。

最終結果:

| 検証 | 結果 |
| --- | --- |
| macOS workspace回帰 | 87成功、0失敗、1 ignored |
| pi-node通常回帰 | 86成功、0失敗、4 ignored、3 filtered |
| 両環境のClippy | 全target、`-D warnings`で成功 |
| 起動・初期化のblack-box試験 | 両環境で9項目成功。通常binaryの投票/確定後にWAL欠損を模擬 |
| Linux専用試験 | 実QUIC構成binding、期限付きcatch-up、TTL/SIGSTOP、bypass、WAL復旧が成功 |
| 4プロセスdaemon試験 | 選出、拒否条件、BAN適用/失効、leader停止/再選出/WAL復旧、少数側と期限切れcatch-up、TLS更新/失効の6段階成功 |
| 終了状態 | 所有daemon終了、XDP残留なし、fixture ping復帰 |
| 静的検査 | Rust整形、差分、Python構文、Markdown 20ファイル成功 |

ignored/filteredは合格数に含めない。Linuxのfilteredは既存の `proof_6_tui_arena`、`proof_7_live_linux`、`regression_r6_r7_linux`。専用試験を別途実行した。追加の同一term投票回帰は、WALを正しく復元したnodeが別候補への二度目の投票を拒否することも確認する。

証拠は `artifacts/wal-bootstrap-5k0brcg3/`。開始前snapshotと差分を保全し、最終source 50ファイルのhashをpi-nodeと照合して不一致なし。Linuxコピーは `/tmp/veil-wal-aKmwb3IW`、`startup-final/result.json` と `flow-final/result.json` に結果を保存した。回収archiveは結果・stdout/stderrのみを選び、秘密鍵を含めない。

初回のコンパイルではRead/Writeの `by_ref` が曖昧だったためReadを明示した。macOSの初回black-box試験では長い証拠pathがUnix socketの上限を超えたため、socketを短い専用directoryへ分離して再実行した。初回証拠回収ではroot所有のflow結果を一般ユーザーで読めなかったため、同じfixture内の結果だけをsudoで再回収した。失敗した記録も保持し、失敗を合格として扱っていない。

この試験は同一Linuxホスト上の4daemon、loopback QUIC、隔離vethのXDP 1か所である。物理NIC、host時計/NTP、既存firewall、本番設定は変更していない。既存checkout・鍵・WALを削除せず、新しいfixtureだけを使った。変更は未コミット。小規模サイトの実遮断導入やBFT全体の保証が完成したという結果ではない。

## 後続の有限BAN必須化

[有限TTLの合意経路全体での強制](2026-10-24-FINITE-BAN-POLICY.md)により、旧形式・TTL=0のBANを新規受信/適用しない。禁止BANを含むWALは保全して起動を拒否する。以前の無期限BAN互換や旧相対TTLだけのWAL復元の記述より、この後続契約を優先する。
