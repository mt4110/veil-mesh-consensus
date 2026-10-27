# 旧アーキテクチャ案と未達事項（終了したロードマップ）

[README](../README.md) · [確認状況・コード根拠](VALIDATION_STATUS.md) · [旧版レビューと改訂案](FEASIBILITY_REVIEW_AND_V1_DESIGN.md) · [シナリオ試験](CONSENSUS_AND_EBPF_PROOF.md) · [過去のクラウド構成案](CLOUD_DEPLOYMENT_GUIDE.md)

2026-10-27訂正。開発終了・本番導入非推奨。本書はかつて目指した設計と公開実装との差を記録するもので、運用保証や開発再開計画ではありません。旧題の「v0.9.1 実践設計書」と完了表記は、その設計全体の達成を意味しません。

## 1. 当初の目的と終了判断

1台が侵害されても、他の健全なサーバーに不当な遮断を波及させない分散防御を目指しました。署名付き状態機械・QUIC・XDPの部品と限定シナリオは試作しましたが、運用としての成立と導入負担に見合う価値は確認できませんでした。

後続の単体ホスト用ローカルBANの隔離・特定hostの成功は、分散防御の達成ではありません。元の用途に対する実測比較がなく、PHP-FPM／DB枯渇の「根本防止」やFail2banに対する優位性を主張しません。

## 2. 旧設計で目指した条件と実際の確認状況

| 旧設計の目標 | 公開実装・記録で確認できる範囲 | 未達・制約 |
| --- | --- | --- |
| 4または7 validator、固定 `cluster.toml` | peerリスト・署名鍵登録・共有ディレクトリの読込みがある | 旧案どおりの固定構成配布・TLS identity・独立host運用の受入れは未達 |
| Prepare／Commitの2段階QCと安全なview変更 | 単一の `QuorumCertificate`、投票lock、TimeoutVote／TCがある | phaseごとに分離した完成BFTではない。一般的な安全性・進行性を証明していない |
| S1：全viewで確定値が一意 | 選択したprefix／競合caseの回帰テストがある | 任意の配送・侵害・復旧を通じた保証は未確認 |
| S2：正しい権限・定足数でのみ執行 | QC・署名検査がある | 正常署名の誤検知を防がない。実kernelの執行成否は別に確認が必要 |
| S3：管理経路・CIDRの自己遮断防止 | exact IPv4の保護集合と一部境界検査がある | 全loopback・管理網・peer・gatewayの自動保護、CIDR交差検査はない |
| S4：daemon停止中もkernelで期限切れ | ユーザー空間の相対TTLと明示時計の試験がある | 公開XDPのABIに期限がなく、この条件を満たさない |
| L1：沈黙／検閲から自動回復 | timeoutを試験から明示的に呼ぶTCの状態遷移がある | 通常daemonのtimer・提案共有・独立hostの検閲回復は未達 |
| IPv4／IPv6のLPM_TRIE | exact IPv4 HASH、ソース上限4096 | IPv6／CIDR／LPMの完成機能ではない。旧案の65536件は実装容量ではない |
| 50ms集約・重複排除・自動容量管理 | 最大128件のbatchコマンドがある | 時間窓入力集約、sensor socket、未失効ルールの安全な回収、負荷性能は別の未達事項 |
| Prometheus HTTP exporter | metrics文字列を生成する関数がある | `127.0.0.1:9100/metrics` や旧案の全SLIを配信していない |
| `veil-node ctl unban`／`emergency-detach` | ユーザー空間のbypass flagがある | 旧CLIは未実装。flagだけで実kernelの遮断は解除されない |

型と通常daemonは[entities](../veil-node/src/domain/entities.rs)・[main](../veil-node/src/main.rs)、kernel ABIは[PeerPolicy](../veil-common/src/lib.rs)・[C版XDP](../veil-ebpf/src/veil_xdp_kern.c)、各処理は[状態機械](../veil-node/src/usecase/consensus_engine.rs)を参照してください。

## 3. BFT・永続化・期限に関する設計上の注意

N=4、必要署名q=3なら、2つのquorumの交差は最低 `3 + 3 - 4 = 2` 台です。悪意あるnodeが最大1台なら少なくとも1台は誠実ですが、この算術だけでは、そのnodeが全phase・view・再起動で矛盾署名を拒否することは証明できません。safe-vote、lock／unlock、新view、鮮度、永続化の契約が別に必要です。

[HotStuff-2著者解説](https://decentralizedthoughts.github.io/2023-04-01-hotstuff-2/)は方式比較の参考です。本作がその完全な状態遷移を実装・検証したという意味ではありません。「約200行でBFT完成」「定足数交差により本実装の安全性を証明」という旧主張は撤回します。

WALは公開版に保存・復元処理がありますが、旧案の「固定256 bytes＋CRCで全必要状態を保持」や「すべての署名応答前にfdatasync」は達成した仕様ではありません。受信時の保存タイミング、checksumと悪意ある改ざんの違い、部分書込み・電源断、投票履歴の巻戻りを区別します。

相対TTLを適用時から開始するだけでは、遅延受信・再起動・再適用で期限が延び得ます。ローカル時刻の不確実性を短期TTLへ置き換えるだけで、元の署名期限を保証できるわけでもありません。旧設計のkernel TTLや期限保証を公開版の機能として案内しません。

LPMの短命な子prefixが期限切れでもmapに残れば、有効な親prefixを隠す設計問題があります。これは[Linux LPM仕様](https://www.kernel.org/doc/html/v6.10/bpf/map_lpm_trie.html)と旧案からの設計上の指摘であり、公開版に存在しないLPM実装の実測障害ではありません。

## 4. 撤回する性能・効果の表現

| 旧記載 | 訂正 |
| --- | --- |
| 合意5〜15ms、同一region p99<25ms | 対象環境・入力・計測結果がない。実測値として撤回。50ms batch待ちを含める遅延のp99<25msという説明も不整合 |
| 秒間2560 subnetを同期 | `128 / 0.05 = 2560` という算術上限。実装・署名・通信・永続化・適用を含む実効性能ではない |
| 容量65536、90%で自動eviction | 旧案の値。公開ソースの4096件HASHと異なる。未失効の確定ルールを回収する方針も運用未検証 |
| 3秒で全復旧 | 復旧CLI・測定結果がない。一般の `ip link ... xdp off` が所有link方式へ適合することも未確認 |
| PHP負荷ゼロ、根本防止、既存手段に対する決定的優位 | 比較試験なし。CDN／LB経路、共有NAT、誤検知、上流帯域は別問題 |
| クラウドVMなら鍵ファイルだけで動作 | kernel・driver・NIC・MTU・権限・PKI・runtimeに依存。試したAWS／GCP実NICでは装着失敗、Azure等は未検証 |

## 5. 外部連携と実運用例

旧図のWAF／Warden／Blackhole→sensor socket→合意→XDP／LSM→veil-rs／Arenaというpipelineは、動作を確認した構成ではありません。`infra/sensor_socket.rs`、`infra/metrics_prometheus.rs`、`scripts/arena-chaos.sh`、`docs/OPERATIONS_RUNBOOK.md` は公開版の完成成果物として存在しません。

CloudflareのHTTPヘッダーに元IPがあることと、XDPが処理するpacketのL3送信元が変わることは別です。CDN／L7 proxyを、元IPへの直接接続と同じ図で防御済みと案内しません。4台のWeb／PHP serverの例は構想であり、公開サイトの受入れ記録ではありません。

## 6. 終了したマイルストーン

| 旧ラベル | 残っている実装・記録 | 旧完了条件の扱い |
| --- | --- | --- |
| v0.2.0／v0.3.0 | 層分離、署名・prefix・鍵mode・apply error等の変更とcase | 「全保証完了」「CIがgreenなので実kernel合格」としない |
| v0.4.0 | TCと明示timeoutの状態機械case | 完成した2段階BFT、fuzzing・独立processの検閲回復まで完了した意味ではない |
| v0.5.0 | 相対TTL・batch型・bypass flag・metrics文字列 | LPM／IPv6、50ms集約、Unix socket、Warden／Nginx自動遮断の完成ではない |
| v0.9.1 | 手動RPC配送・明示WAL復元の複合case | 独立hostのnetem・鍵rotation・公開pilotの完成ではない |

[CHANGELOG](../CHANGELOG.md)の版番号は当時の変更区分です。2026-10-27に確認した公開Releaseはv0.2.0までであり、v0.9.1のRelease公開を示す表ではありません。旧計画の全完了条件との一致を保証しません。設計・製品開発・追加課金試験は終了しており、この表を次の実装依頼として扱いません。

## 7. 記事で参照する根拠

[公開コードと試験の確認状況](VALIDATION_STATUS.md)、[初期レビューの対象版と反例](review/2026-10-23-REPRODUCTION.md)、[後続ローカル版のクラウド試験](review/2026-10-27-CLOUD-VALIDATION-SUMMARY.md)を、対象と限界を添えて参照してください。旧設計の目標や機能名だけを、成功した実装・採用価値の根拠にしないでください。
