# 一覧・手動適用・解除

[README](../../README.md) · [導入](INSTALL.md) · [復旧](RECOVERY.md) · [試験前チェック](PILOT_CHECKLIST.md)

以下は、配置と変更範囲を承認した同一hostの例です。pathは標準unitの`primary`を使用します。手動復旧で`recovery`へ切り替えた場合は、**すべての管理commandを同じrecovery pathへ切り替えます**。directoryやUIDを混在させません。
管理clientはdaemonと同じ専用UIDで実行します。rootのままsocketへ接続しても、同じUIDでなければ拒否されます。

## 1. 状態と一覧を見る

```sh
sudo -u veil-local-ban /usr/libexec/veil-local-ban status \
  --control-dir /var/lib/veil-local-ban/primary
sudo -u veil-local-ban /usr/libexec/veil-local-ban list \
  --control-dir /var/lib/veil-local-ban/primary
```

`mode=local-ephemeral`、`capacity=256`、instance、revision、scopeを確認します。一覧は最大256件の読取り専用snapshotです。

| field／状態 | 意味 |
| --- | --- |
| active | snapshot時点で未失効、最後のMap挿入を確認済み |
| pending／confirmed=false | 挿入結果を確認できていない追跡slot。成功扱いにしない |
| expired | 期限切れ、回収前のslot。まだ容量を使うが、その期限を過ぎたBANではDROPしない |
| remaining_secs_ceil | 残り秒数の切上げ表示。絶対UTCではない |
| source=userspace-mirror | 管理状態の表示。実通信の遮断証明ではない |
| bypass_observed | 別途観測した値。BAN snapshotと同時に固定したcontrol revisionではない |

`BUSY`を空一覧と扱いません。statusが`policy_busy=true`のときも、revisionや件数を得たと仮定しません。必要なら読取りだけを再確認し、変更命令は自動再送しません。
pendingやMap errorがあれば状態を調べ、必要なら[停止・復旧](RECOVERY.md)へ進みます。

## 2. BANをプレビューする

BAN前に、問題の根拠、実L3送信元、共有NAT等への巻込み、対象service、有限TTL、解除経路を人が確認します。HTTP headerのIPだけでXDPの対象を決めません。
以下のIPは文書用で、実際には所有・影響を確認したsourceへ置き換えます。

```sh
sudo -u veil-local-ban /usr/libexec/veil-local-ban preview-ban \
  --control-dir /var/lib/veil-local-ban/primary \
  --ip 198.51.100.20 --ttl-secs 30
```

previewは対象IP・保護IP・有限TTL・容量等を確認し、現在のscope、既存期限、操作のeffectを表示します。
`replace-existing-lease`は同じIPの期限更新です。無意識に延長しないでください。TTLは1〜86,400秒で、実際のban実行時から数えます。
previewは予約ではなく、rate制限・競合・Map操作の成功を保証しません。件数・期限・revision・eventを変更しません。

## 3. 確認したコマンドを別途実行する

`execute_manually=`の右側に、対象・instance・expected-revisionを固定したcommandが出ます。**目で照合してから、同じservice UIDで一度だけ実行**します。sudo運用なら右側のcommandにも`sudo -u veil-local-ban`を前置します。
生成commandの引数を保ったまま実行し、`eval`、shellへのpipe、自動cron／queueには渡しません。絶対pathを使い、host・UID・control-dirも変えません。

別の変更でrevisionが進めば拒否、restartでinstanceが変われば拒否されます。tokenだけ自動取得し直して通しません。
mutationもrestartもなければ、previewは時間だけでは失効しません。古くなった表示・操作意図は再確認します。

`APPLIED`はMap syscall成功です。対象clientのHTTPSが遮断され、独立正常client・管理経路・対象外通信が維持されることまで確認します。kernel drop counterは補助であり、それだけで対象IPの効果を証明しません。

## 4. 解除・失効を確認する

```sh
sudo -u veil-local-ban /usr/libexec/veil-local-ban preview-unban \
  --control-dir /var/lib/veil-local-ban/primary --ip 198.51.100.20
```

このpreviewも読取り専用です。出力したunban commandを前節と同じ方法で別途実行します。
解除直前にまだ未失効・遮断中であることを確認し、実行後に対象通信が復帰することを確認します。自然失効を手動解除の成功に数えません。
自然失効の試験では元のkernel deadlineを記録し、再BAN／延長で条件を変えません。回収待ちentryが残ることと、遮断が続くことを混同しません。

## 5. 失敗・競合・結果不明を区別する

| 結果 | 次の行動 |
| --- | --- |
| CAPACITY | 新IPは追加しない。既存の有効BANを追い出さず、失効／回収状況を見る |
| REVISION_CONFLICT／INSTANCE_CONFLICT | 古いcommandを破棄し、現在の状態と操作意図を再確認。古い要求を新tokenで自動再送しない |
| RATE_LIMIT／BUSY | 無制限queueを作らない。変更を自動retryしない |
| MAP_INSERT_FAILED／MAP_DELETE_FAILED | 成功とも未適用とも断定しない。status／listと実通信を確認し、必要なら停止 |
| OUTCOME_UNKNOWN_CHECK_STATUS／timeout | workerで適用される可能性が残る。再送せずstatus／listと実通信を確認。復旧を確認できなければ停止 |

CLIは非OK応答で非ゼロ終了します。標準出力の一部やexit codeだけでpacketの結果を決めません。
通常管理は8枠、mutationは1件、管理I/Oは2秒。変更がtimeoutしても内部worker完了までは受付枠を保持する契約です。

## 6. bypass・停止・再起動

bypassは別の緊急socketを使い、BAN用の`revision`とは別の**`control_revision`**を要求します。新instanceと現在値はstatusから確認します。
下のREPLACE値はそのままでは無効です。表示値を人が確認して置き換える例です。

```sh
sudo -u veil-local-ban /usr/libexec/veil-local-ban bypass \
  --control-dir /var/lib/veil-local-ban/primary --mode on \
  --instance REPLACE_INSTANCE --expected-revision REPLACE_CONTROL_REVISION
```

bypass on後も実通信で復帰を確認します。bypass offは未失効BANを再び効かせるため、影響を確認し、新しいcontrol_revisionで明示操作します。
通常socketの接続枠が塞がる場合、status commandも通常socketを使うため取得できないことがあります。緊急socketを使うbypass自体と、必要なtokenを通常CLIで取得できることは同じ保証ではありません。状態を確認できなければ[unit停止](RECOVERY.md)を使い、推測したtokenを送りません。

追加遮断全体を止める場合:

```sh
sudo systemctl stop veil-local-ban.service
```

別の承認した場面で、通常再起動する場合:

```sh
sudo systemctl restart veil-local-ban.service
```

stopは所有XDPを外し、restartはBANを全消去して新instanceになります。旧command・旧一覧からBANを復活させません。
停止・再起動後の通信とXDPを確認する方法は[復旧手順](RECOVERY.md)へ。起動・停止・restartは配備対象のunitだけに限定します。

詳細なwire／上限／実装時回帰は[一覧・preview契約](../review/2026-10-25-LOCAL-BAN-OPERATOR-LIST-PREVIEW.md)、実XDPの確認範囲は[受入れ報告](../review/2026-10-25-LOCAL-BAN-OPERATOR-KERNEL-ACCEPTANCE.md)を参照してください。

## 7. 机上確認の記録（2026-10-25）

利用者本人とこのチャットで、保存済みの試験出力を順に読み、次の6場面を確認した。各場面で本人の回答を受け、必要な補足をしてから次へ進んだ。**判断の言語化を確認した記録であり、無援助の実機操作・所要時間・誤操作率を測った試験ではない。** この確認中にcommand実行・設定変更・BAN操作は行っていない。

| 場面 | 本人の回答で確認した判断 |
| --- | --- |
| 起動直後の一覧 | entries=0はBAN0件、capacity=256は上限。対象は10.99.0.66のTCP18443。この表示だけでHTTPS成功とは判断しない |
| BANのpreview | まだ適用されていない。TTL30秒は実際の適用時から開始する |
| APPLIED | 応答だけで通信への効果を断定せず、対象の遮断と対象外通信の継続を実通信で確認する |
| REVISION_CONFLICT | 解除要求は拒否。revisionだけを変更して再送せず、現状と意図を確認し、必要なら新しいpreviewを取る |
| INSTANCE_CONFLICT | 再起動前のcommandを新しいprocessへ適用させないための拒否。instanceだけを置換せず、空で始まる契約を踏まえBANの必要性から再判断する |
| OUTCOME_UNKNOWN_CHECK_STATUS | 適用済みの可能性を残し、同じcommandを再送しない。一覧・状態と実通信を確認し、確認不能なら対象unitを止め、XDP非装着と通信復帰まで確認する |

最初の5場面は[保存済み実XDP受入れ](../review/2026-10-25-LOCAL-BAN-OPERATOR-KERNEL-ACCEPTANCE.md)の一覧・preview・command応答を使用した。
最後の結果不明caseはその実XDP実行では発生していない。既存のLinux library回帰`timed_out_mutation_keeps_single_admission_until_worker_finishes`と本手順に基づく**想定問題**として区別した。模擬Backendの回帰では、応答timeout後もworkerが受付枠を保持し、後で適用される動作を確認している。利用者本人がその異常を実機で操作した証拠ではない。

机上確認は完了。この確認から必須の追加実装は出ていない。実運用者の操作の迷い・所要時間・実機での復旧操作は未検証のまま、[環境準備後の再開位置](PILOT_CHECKLIST.md#再開位置2026-10-25)へ引き継ぐ。公開siteの受入れ欄を、この回答だけでチェック済みにしない。
