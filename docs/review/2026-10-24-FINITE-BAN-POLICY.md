# 有限TTLを合意経路全体で必須にする

最終ゴールは小規模サイトでの安全な試験導入。今回はIPv4のIP BANについて、期限なしの遮断を提案・受信・適用・WAL復元で禁止する。以前のTTL=0互換動作と、旧形式の相対TTLログを復元して抑止する動作は、この記録で置き換える。

## 受理する命令

| 経路・形式 | BAN（trusted=false） | 解除（trusted=true） |
| --- | --- | --- |
| `SetPeerTrust` | TTLも共通期限もないため拒否 | 受理 |
| ローカル提案の `SetPeerTrustBatch` | TTL 1〜86,400秒のみ受理し、署名前に共通期限付きへ変換 | TTL=0も受理 |
| RPC/WALの `SetPeerTrustBatch` | 相対TTLが正でも、署名対象の共通期限がないため拒否 | BANを含まないbatchは受理 |
| `SetPeerTrustLeaseBatch` | 正のTTLと、`not_after = issued_at + TTL` を満たす共通期限が必須 | TTL=0・not_after=0の解除を受理 |

batch内に1件でも不正なBANがあれば全体を拒否する。署名、構成binding、QCが正しくても、この方針を上書きできない。保護IP、TTL上限、overflow、batchサイズの既存検査も維持する。

ローカルの提案入力と、ネットワーク/履歴上の命令を別の検査へ分けた。`validate_command` は相対TTLのローカル入力を許す。`validate_log_command` は共通期限を持つログ形式だけを許し、提案の期限付与後、受信ログ、適用前、WAL全体の検査に用いる。

適用直前にも、BANのTTLと準備済みboot deadlineが0でないことを検査する。既存の期限切れtombstone=1は失効状態であり、新しい遮断として適用しない。期限後catch-up、再試行、再起動によって期限を延ばさない仕組みは維持する。

## WAL復元と移行

禁止されたBANを含むWALは**ファイルを変更せず、validatorの起動を拒否する**。未確定部分や、後続の解除で上書きされる履歴も対象。過去の投票・hash・QCを消したり、期限を後付けして再署名したりしない。

検査は全ログについて、復元stateの設定・backendへのルール適用より前に行う。失敗は停止状態として保持し、呼出し元が復元エラーを無視しても、新しい選挙・投票へ進めない。通常daemonではエラー終了する。XDP backendを指定した起動では、WALの内容検査に先行して新しいbypass状態のmapをattachする場合があるが、禁止ルールを適用せず終了時に所有linkを解放する。

有効な共通期限付きBANだけを含む既存WALは、元の署名context・hash・投票履歴・期限のまま復元できる。期限切れの共通期限付きBANは有効な履歴であり、起動拒否せず、従来どおり遮断を再開しない。

WAL形式・構成digest・TLS SANは変更していない。有効な有限BANのwire表現を変える変更ではなく、以前許可していた危険な入力を拒否する修正である。旧binaryとの混在を安全な配備として扱わない。全validatorを停止して更新・検査し、禁止WALが見つかったnodeは停止を維持する。旧nodeは引き続き無期限BANを受理し得るため、更新済みnodeの拒否がクラスタ全体への更新を代替することはない。

禁止WALの安全な移行ツールは今回実装していない。WAL削除、`--init-state`、鍵再生成、問題のentryだけを削る操作で復旧してはならない。[WAL初期構築・復旧の境界](2026-10-24-WAL-BOOTSTRAP-RECOVERY.md)を維持する。旧試験fixtureや履歴は削除せず保全する。

## 検証の意味

- 旧単体BAN、TTL=0 batch、相対TTLだけの受信BAN、TTL=0の共通期限付きBAN、不正BANと解除が混ざったbatchを拒否する。
- 正規署名・検証可能な定足数QCを付けた拒否試験でも、ログ・投票・WALが不変で、QC投票署名を返さない。
- 適用済みとして不正ログを直接注入した防御試験でも、backendへ書き込まず、適用cursorを進めない。
- 禁止WALは確定/未確定の両方で拒否し、元ファイル不変、backend書込みなし、復元失敗後の選挙/timeout投票停止を検査する。
- 実QUICでは拒否ケースだけでなく、有限BANの合意・適用と、期限なし解除の成功を対照条件として確認する。
- Linux専用試験は既存の隔離 `veth-victim` と `ns-veil-rogue` のみを使う。拒否ケースと禁止WAL復元時は実packet PASS、有限BANではDROP、解除後PASSを確認する。

TUIのBAN提案も有限TTLへ変更した。実カーネル接続モードではLinuxのboot clockを使い、設定に失敗した場合は提案を止める。tickで期限状態も更新し、シミュレーションのboot=0を実カーネルの期限へ渡さない。対話型TUIの全操作を実機で受入れたという検証ではない。

既存テストの正常系入力は有限TTLへ変更した。署名・競合QC・二重投票・WAL復元等のassert条件は弱めていない。既存の「旧相対TTLだけのWALは履歴を復元して再武装しない」試験は、新契約である「元WALを保全し、復元と投票を拒否する」試験へ変更した。

この変更は合意エンジンを経由するIP BANの契約であり、rootがBPF mapを直接操作する経路や、別命令のLSM lockdownへ期限を追加する変更ではない。TTL満了時刻や複数nodeの同時解除の保証も拡張していない。

## 最終検証結果

| 検証 | 結果 |
| --- | --- |
| macOS workspace回帰 | 92成功、0失敗、1 ignored |
| Linux通常回帰 | 92成功、0失敗、5 ignored、3 filtered |
| 両環境Clippy | 全target、`-D warnings`で成功 |
| 有限BANの実QUIC | 禁止命令5パターンを拒否し、有限BANと解除は成功 |
| Linux専用の実packet検査 | 禁止BAN/禁止WALでPASS、有限BANでDROP、解除後PASS。最終観測はPASS=7・DROP=1 |
| 既存Linux専用試験 | 構成binding、署名期限catch-up、SIGSTOP失効、bypass、WAL復元が成功 |
| WAL初期構築black-box試験 | 既存の9項目が成功 |
| 通常daemon 4プロセス | BAN適用/失効、再選挙、catch-up、TLS更新/失効を含む6段階成功 |
| 終了状態 | daemon終了、XDP残留なし、fixture ping復帰 |
| 静的検査 | 整形、差分、Markdown 21ファイル成功 |

ignored/filteredを合格数に含めない。Linuxのfilteredは既存のTUI・環境依存試験3件。新しい有限BANのLinux試験は明示実行した。TUIのclock設定を追加した後、通常回帰/Clippyと有限BANの実packet試験・通常daemon通し試験を再実行した。対話型TUIの全操作の受入試験ではない。

証拠は `artifacts/finite-ttl-w3z91wmq/`。Linux検証コピーは `/tmp/veil-finite-s0N7BGSS`、最終daemon結果は `flow-confirmed/result.json`。実機ソース51ファイルのhashを照合し不一致なし。禁止WALと正常WAL、結果JSON、実行ログを保全し、回収archiveへ秘密鍵を含めない。

実機試験は同一Linuxホストのloopback QUIC、隔離vethのXDP 1か所。物理NIC、host時計/NTP、既存firewall、本番設定を変更していない。既存ファイルの削除、commit/push、公開は行っていない。既存の選挙・BFT進行性・容量・site受入れの課題を完了したとは扱わない。
