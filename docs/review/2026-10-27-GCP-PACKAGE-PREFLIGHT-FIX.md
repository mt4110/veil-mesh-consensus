# GCP試験packageの配置属性・非root事前確認を追加課金なしで修正

2026-10-27 JST。Sol／High、分担なし。本人の指示で、[前回の配置権限失敗](2026-10-27-GCP-VM-BOOT-AND-EXEC-PERMISSION-FAILURE.md)を試験側だけで修正・検証した。GCP API・SSH・有料resource作成・設定変更なし。次回の[一台再試験案と費用・撤去](../local-ban/GCP_PACKAGE_RETRY_PLAN_20261027.md)は未承認・未実行。

最終ゴールは、普通のブラウザが使う小規模公開サイトへの安全な手動一時BANの限定導入。GCPは導入先候補であり、今回のゴールは**誤った配置を事前に拒否し、正しい非root読取り・実行権・書込み不能を確認すること**。この修正は完了したが、GCPの製品適合と公開六条件は未達。

## 変更と維持した範囲

- [package helper](../../scripts/local-ban-trial-package.py)は、source uid／mode／ACL等をコピーせず、新規USTARのroot:root・binary0755・object等0644を固定する。既存hashを照合し、出力は新規作成限定。旧archiveや元の700／600をchmodしない。
- [access helper](../../scripts/local-ban-trial-access.py)は、guestの親dir・通常ファイル・単一link・root所有・実mode・hashを検証し、非root／NoNewPrivs／全capability0で実際に読取り、実行権と書込み不能を確認する。
- service-checkは確認成功後に限り、専用DynamicUser・全capabilityなし・PrivateNetwork・Restart=no・15秒上限のTransient unitで**実際の製品--help**を要求する。--help失敗・timeoutを通過扱いにしない。serveやBPFを実行する機能は持たない。
- 製品binary・object・既存隔離driver・本体service profileを変更しない。期限・TTL・256件・再起動でBAN0・instance／revision・180秒／240秒は維持。nftables・BFT・自動BANの作業を再開しない。

## 検証結果と限界

| 検証 | 結果 |
| --- | --- |
| Python回帰24件 | 合格。旧実archiveの拒否、700／600、誤owner／group、書込み可能／setuid、link・重複・path escape・PAX、改ざん・欠損・不明hash、既存出力保全、--help失敗／timeout、rootでの非root代替拒否等 |
| 実package buildと再verify | 合格。既存製品・object・driverの内容hash同一、root0755／0644・余分なmemberなし |
| Linuxの旧権限反例 | UID/GID65534でroot700 binary・root600 objectが読めず、binaryの実行権もないことを実確認 |
| Linuxの修正版アクセス | 同UID/GID・NoNewPrivs・全capability（bounding含む）0で6ファイルを読みhash照合、binaryの実行権あり、package書込み不可 |
| 実書込みの拒否 | 非rootでbinaryとobjectをO_WRONLYで開く操作が両方PermissionError。内容は書き込まない |
| systemd unitとx86_64製品--help | **未実行**。ローカルLinuxはARM64で、GCPでの必須ゲートとして残す |
| GCP guest・BPF・実NIC・BAN・公開経路 | **今回未検証** |

Python結果は[unit-tests-v2.log](../../artifacts/gcp-package-fix-ku098if1/unit-tests-v2.log)、修正版は[payload-v2.tar.gz](../../artifacts/gcp-package-fix-ku098if1/payload-v2.tar.gz)と[manifest・hash](../../artifacts/gcp-package-fix-ku098if1/package-v2-result.json)、実Linuxの結果は[linux-access-c-result.json](../../artifacts/gcp-package-fix-ku098if1/linux-access-c-result.json)。根拠のない全環境合格にはしない。

## ローカル試験の最初の失敗も保全

既存Colima内のcached Python ARM64 imageを使用し、新規imageは取得していない。network=none、root filesystemはread-only、cap-drop=ALL、入力bindはreadonly、memory256MiB／PID32／CPU1で試験した。ホストのDocker socketをcontainer内へ渡していない。

最初のlocal caseはDockerのtmpfsがnoexecで、修正版binaryの実行権判定を拒否した。**この失敗を隠さず保全**。[原結果](../../artifacts/gcp-package-fix-ku098if1/linux-access-result.json)・[実mountの診断](../../artifacts/gcp-package-fix-ku098if1/tmpfs-diagnostic.log)。[Docker公式仕様](https://docs.docker.com/engine/storage/tmpfs/#options-for---tmpfs)を確認し、次の新規local fixtureだけをexec,nosuid,nodevのtmpfsへ変更。これは最初からexec可能だったGCPの/runを再現する条件で、GCP・hostのmountやsandboxを変更したものではない。再現可能な原因を確認してから比較した。

初版packageの比較、最終helper追加後の再比較、診断を合わせて今回新設した四つのlocal containerは全て停止・保持。自動removeや削除はしない。tmpfsの試験データは停止で失われるため、結果は先にprivate artifactsへ保存した。[四containerの終了状態](../../artifacts/gcp-package-fix-ku098if1/containers-final-v2.json)。既存container・image・daemon設定は変更していない。

## 保全・残り・終了条件

旧archive・元の失敗result・前回実行要約／報告・製品source／object／profile等10ファイルをhash・mode・mtimeで照合し、不変を確認。[保全記録](../../artifacts/gcp-package-fix-ku098if1/preservation-result.json)。sourceの秘密鍵・tokenを読み出していない。

今回の修正と追加課金なしの検証は完了。残りは、①新規承認の一台でservice-check→隔離機能→実NICと管理維持・解除・撤去を確認、②合格した場合だけ別承認の公開六条件と導入価値を確認。次回案は最大60分・運用上限US$0.20・税前見積りUS$0.06、T0＋45分DELETE。前回の未達や今回のlocal成功を公開導入完了へ読み替えない。

モデル／推論はgpt-6.1-sol／high、分担なし。入力・cache入力（入力の内数）・出力使用量は未取得。今回の[要約](../../artifacts/gcp-package-fix-ku098if1/result.json)と[文書検証](../../artifacts/gcp-package-fix-ku098if1/document-verification.json)を保持する。

[試験履歴](../local-ban/TEST_HISTORY.md)へ戻る。
