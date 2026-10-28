# 認証付きチェックポイント・状態転送・WAL容量上限

最終ゴールは小規模サイトへの安全な試験導入。今回の完了範囲は、ユーザーが選択した恒久対応（認証付きチェックポイント、状態転送、容量上限、回帰とpi-node確認）。容量を超えて止めるだけの対策とは区別する。旧二段階方式の記録より本書を優先する。

## 保存と通信の契約

通常daemonはconsensus context 5、WAL v8を使用する。cluster ID・epoch・構成digest・固定membership・ノード別TLSは維持する。旧WAL v4〜v7の自動変換は行わず、元のファイルを保全して起動を拒否する。同じ署名鍵で空の履歴を作り直して移行してはならない。既存クラスタの移行手順と新旧binary混在は今回の対応範囲外。

| 制約 | 値・動作 |
| --- | --- |
| checkpoint間隔 | 32確定entry。BANだけでなくNoOp・整理命令も数える |
| 未認証の保存窓 | 最後のstable checkpointから最大32 entry。新しい証明が揃うまで次の窓に進まない |
| snapshot状態 | 最大256 BANレコードと固定member分のLSM状態、認証済み整理時刻 |
| snapshot byte上限 | 32 KiB。単一の認証済みRPC内で転送し、途中の受信データをWALへ書かない |
| 通信上限 | 従来どおり64 KiB。member最大7、command最大1 KiB |
| WAL byte上限 | 既定16 MiB。`--max-wal-bytes`で64 KiB〜256 MiBのローカル予算を指定できる |
| 上限到達・保存不確定 | 署名応答・投票を停止してエラーをラッチ。直前のWALを勝手に消さず、復旧を要求する |
| 状態表示 | `--status-local`に`wal_bytes`、`wal_limit_bytes`、`wal_remaining_bytes`、`checkpoint_height`、`retained_entries`を追加 |

WAL上限はクラスタの合意値ではなく各ノードの資源予算。小さすぎる設定は可用性を落とす。起動時にも大きさを確認し、読み込み・decodeに上限を設ける。安定した運転では履歴長ではなく状態量と未整理窓が容量を決める。攻撃や障害で同じ未確定位置のview交代が続く場合の投票履歴は、byte上限で止める。

## 認証と整理の順序

1. 各ノードは確定・適用済みの32件境界で、最後のstable状態と保持中の確定ログから、時計に依存しない同一の論理状態を計算する。
2. `VEIL_CHECKPOINT_V1`、署名context、height、prefix hash、状態digest、voterを結合して署名する。自分の署名済みcheckpoint票をWALに保存するまで送信しない。
3. 各memberが票を交換する。leaderだけを票の中継役にしない。`floor(2N/3)+1`の異なる固定voterが、同じheight・prefix・状態digestへ署名した証明と、その位置のcommit QCを検証する。
4. 認証済み状態、下限位置、anchor commit QC、必要なローカル期限・バイパス、残存suffix、未解決prepare/投票・交代報告を一つのWALへ保存する。成功後に応答・適用を進める。
5. 下限以前のログと各entryのQC・投票を、認証済み状態と永続化された下限へ置き換える。下限より先の投票・prepare証明は残す。古い位置へのprepare/commitを再署名しない。

旧ファイルと新しい下限を別々に更新しない。既存のtempへの書込み・file fsync・atomic rename・directory fsyncを使用する。保存前失敗なら旧WAL、rename後に成功が不明なら旧または新WALのどちらからでも、安全な境界を復元できなければならない。

書込み失敗で残る専用tempも自動削除しない。同じWALの`.<pid>-<serial>.tmp`を検出したら、追記・再起動時の保存を拒否し、障害の繰り返しでtempを積み増すことを防ぐ。運用者は原本とtempを保全・隔離して原因を調べる。tempを新しい正本として自動採用しない。無関係な`.tmp`は変更しない。容量上限は正本と一回のtemp書込みに対するもので、過去の証拠・バックアップ・他アプリのディスク消費を管理するquotaではない。

[PBFT原論文 §4.3・§4.4](https://www.usenix.org/legacy/publications/library/proceedings/osdi99/full_papers/castro/castro_html/node4.html)のstable checkpoint・状態認証・watermarkを参考にしている。原論文全体の実装や形式検証済みの主張ではない。

## 状態転送・復旧・期限

- 遅延ノードや遅れた新leaderが、すでに整理された位置を要求した場合はsnapshotを送る。送信者の申告だけを信頼せず、内包QC・定足数署名・状態digest・policy・サイズを検証する。
- 認証前の入力ではログ・投票・WAL・kernel状態を変更しない。古いsnapshotで現在のcommit位置を巻き戻さない。同じ位置ならローカルで計算した確定状態との一致も要求する。
- checkpointはleaderの証明ではない。ログ同期後は従来のnew-view証拠でleaderを認識し、fresh challengeを行う。
- snapshotのBANは元の発行時刻・絶対期限・元の適用位置を保持する。他ノードのboot時計の期限を輸入しない。初めて受け取った判断だけを自ノードの誤差・速度制限で変換し、保存してからkernelへ適用する。
- 既知の判断には保存済みの短い期限・失効tombstoneを維持する。既知のcommitに期限記録がなければ失効扱いとし、復活させない。別bootでのWAL復元も失効側にする。ローカル緊急バイパスは他ノードのsnapshotで上書きしない。
- snapshot適用が途中で失敗したら、成功したと報告せず再試行する。再試行で期限を計算し直さない。WAL復元時は後続ログで上書き済みの古いポリシーを一時的に復活させない。
- すでに整理された個別indexは、statusで`compacted`を返す。個々の過去entryの証拠や監査ログを恒久保存する機能ではない。保持中のwitnessed要求については、ローカルの確定receiptも保存し、同じ要求を再提案しない。

## 期限切れ状態自体の増大対策

時計だけを見て各ノードが別々にBANレコードを消すと、snapshotのdigestが一致しない。そのため、leaderはidle時に`RetireExpired { through_unix_ns }`を通常のprepare/commit経路へ提案する。

各voterは、整理時刻が自分の`UTC時刻 − 誤差上限`以下であることを確認してから票を出す。未来の時刻、時刻品質が不明な状態では整理へ投票しない。確定した整理時刻は単調増加し、そこまでに絶対期限が切れたレコードだけを論理状態から除く。checkpointへ整理時刻も含め、過去の期限切れコマンドを受けても状態のレコードを再生しない。packetの期限失効自体は、引き続きkernelのboot時計で実行される。

最大256件は、未失効またはまだ整理が合意できていないBANの上限。無制限の同時BANを保証しない。上限時は新しいIPの提案を拒否し、既存BANを容量確保のために勝手に解除しない。時刻同期や定足数が復旧すれば、整理命令で容量を回収できる。明示的なunbanも既存の合意コマンドとして状態のslotを解放する。

## 検証記録

証拠: `artifacts/wal-budget-xc1cmy41/`。開始前のsource・差分を保存。実行中の失敗ログも残し、未確認を成功と扱わない。

| 検証 | 今回の結果 |
| --- | --- |
| macOS全体回帰 | 147 passed、0 failed、1 ignored。`mac-full-6.log` |
| pi-node全体回帰 | 147 passed、0 failed、6 ignored、3 filtered。専用Linux/root試験を分離し、test-threads=2。`linux-regression-final.log` |
| 新しいcheckpoint専用回帰 | pi-nodeで14 passed。全体147件に含まれるため合算しない |
| build・Clippy | pi-node buildと、両OSのClippy（warningsをエラー扱い）が通過 |
| 書式 | rustfmt、Markdown lint、Python構文、diff whitespaceを確認 |
| 通常daemonのWAL起動経路 | pi-nodeで欠損・破損拒否、同じ鍵での再初期化拒否、正しいWALからの復元等9項目通過。`linux-startup.log` |
| source同一性 | build対象68ファイルのSHA-256をローカル／pi-node間で照合、一致。`linux-source-and-cleanup.log` |
| 実BAN／XDP | 承認された一度の時刻同期サービス再起動後、実TTL・bypass・WAL復元と、競合prefixからの合意再開・XDP DROP→期限失効PASSが通過。`linux-acceptance-final.log` |
| 実daemonでのcheckpoint・状態転送 | `scripts/daemon-flow-check.py --checkpoints`の全8段階が通過。4独立process・ノード別TLS・通常CLI・実XDPを使用。`linux-flow-result.json` |
| 独立した後状態 | XDP attachmentなし、隔離fixtureのping成功、残存veil-nodeなしを確認 |

初回Linux全体回帰の並列実行では既存の実QUICテストがtimeoutした。ログを保全し、同じ期待条件のままtest-threads=2で全体を再実行して通過した。実TTL試験の最初の試行はkernel時計誤差が既定1秒を超えたため安全側拒否で停止した。ユーザーの明示承認後、`sudo systemctl restart systemd-timesyncd`を一度だけ実行し、NTP応答の`Ignored=no`とアプリの時刻品質検証を確認して再試行した。NTP設定・アプリの誤差上限は変更していない。再起動は試験再開のための対応で、長期の時刻同期品質を保証するものではない。

測定値はfixtureに限る。4ノード・32 NoOpのWALは15,314→1,891 bytes。128件を4回整理したstable WALは各1,891 bytes。512種類の有限TTL BANを整理するシミュレーションでは最大13,990 bytes、最後13,098 bytes。最大7メンバー・256ルール・最大長の名前と証明を構成したsnapshot wire fixtureは18,498 bytesで、64 KiB以内だった。実サイト負荷や長時間のサイズ保証の実測ではない。

実daemon試験ではnode bを停止し、残る3ノードが高さ32・64でそれぞれWAL 5,508 bytes、保持entry 0件へ整理した。node bは高さ64の認証付き状態を取得し、期限切れBANを復活させずに復帰。XDP所有node dのWAL再起動後もPASSを確認し、その後index 66の新規BANが全4ノードで確定・適用され、実DROPから期限失効PASSへ戻った。終了後に別コマンドでXDP attachmentなし、veil-node残存なし、隔離fixture ping成功、ソース68ファイル一致を確認した（`linux-acceptance-poststate.log`）。

受入れスクリプトは境界receiptの`compacted`表示を認識する。ただしreceipt hashとcheckpoint境界hashの一致、commit・last_appliedの到達、backend状態を引き続き要求する。境界より古く個別hashが失われたentryを、整理済みという表示だけで合格にしない。

チェックポイント後のindex 33に未配送commit QCがある場合も、WAL復元と2回のview交代で同じ値へ確定した。kernel期限失効の削除失敗を注入し、checkpoint後にも再試行記録が残って、backend復旧後に削除できることを確認した。

対象回帰は、定足数未達での窓停止、繰返し整理時の容量、古い投票再送、遅れたcandidate、認証不正・構成違い、保存失敗前後、期限・bypass、要求receipt、状態件数上限、失効整理、temp保全を含む。

今回の恒久対応は履歴由来の増加を抑えるもの。独立した形式証明、長時間運転、複数ホストでの分断・時計障害、旧クラスタの移行、実サイトでの運用受入れは未完了。小規模サイトへの投入可否は引き続き未判定。
