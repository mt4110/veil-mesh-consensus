# 公開コード・試験記録・過去の構想の確認状況

2026-10-27。開発終了、本番導入非推奨。記事から参照する場合の、公開文書の確認範囲と制約をまとめます。

## 対象と証拠の読み方

- **公開コード**：文書訂正前の `0c58d7179f0616b92893176d24a697330791f503`。今回の訂正は文書・説明図のみで、製品コード・配布object・CI設定を変更しません。
- **初期レビュー**：`c8d3f7f` を対象にした[再現記録](review/2026-10-23-REPRODUCTION.md)。その版の反例と、後続版で追加された修正・テストを区別します。
- **後続ローカル試験**：公開mainと同一でない単体ホスト用 `veil-local-ban` の[AWS・GCP要約](review/2026-10-27-CLOUD-VALIDATION-SUMMARY.md)。公開mainの分散メッシュ全体が動いた証拠には使いません。
- **過去の構想**：旧設計書、クラウドの手順例、他製品との組合せ。実装・試験・運用保証としては扱いません。

本書のコードに関する記述は静的確認です。今回、Rust試験の再実行、Linux／クラウド／実NIC／本番試験、依存脆弱性の網羅監査はしていません。テストの存在、過去の合格報告、今回の実行結果を混同しません。生ログを公開していない結果は、この文書だけで第三者が独立検証できるものではありません。

## 公開コードにあるものと、そこから言えないこと

| 対象 | 確認した実装・試験の範囲 | 未実装・未確認・保証しないこと |
| --- | --- | --- |
| Ed25519署名・QC・prefixハッシュ | 署名付きRPC入口、QC検査、`digest_with_parent`、限定した攻撃ケースの回帰テストがある | 任意の攻撃・配送順序・侵害ノードに対するBFT安全性／進行性の証明ではない。署名や3票だけで入力の真実性・誤検知防止は得られない |
| mTLS・共有PKI | `ClusterMtlsPki` と共有ディレクトリのCA・鍵読込み、loopback QUICのテストがある | PKIテストは独立プロセスの本番クラスタ全体の受入れではない。通常daemonが共有CA秘密鍵を読み得る構成や自動公開鍵探索を、運用上安全な鍵配布と呼ばない |
| 通常daemon | Ping、RPC受信、条件付きAppendEntries送信、WAL読書きがある | 自動選挙・timeout駆動・sensor／管理API受付を備えた完成した運用runtimeではない。peer IDと実node IDの対応も運用受入れ未達 |
| View-Change | `TimeoutVote`／`TimeoutCertificate` と `trigger_view_timeout`、`proof_12` がある | 試験から明示的にtimeoutを呼ぶ状態機械のシナリオ。通常daemonの自動timer・提案再送・検閲回復を実証していない。独立したPrepare／Commit phaseのQCを持つ完成BFTでもない |
| TTL | `PeerTrustRule.ttl_secs`、ユーザー空間の期限map、時計を明示的に進める `proof_13` がある | 公開 `PeerPolicy` とXDPに失効時刻フィールド／kernel期限判定はない。daemon停止中のkernel自律失効、通常daemonの実時計駆動、復元後に期限を延ばさない保証はない |
| 緊急バイパス | `set_emergency_local_bypass` が `check_xdp_packet` のユーザー空間判定を変える | kernel mapや所有XDP linkを解除する運用機能ではない。`veil-node ctl unban`／`emergency-detach` CLIや3秒復旧は未実装・未測定 |
| Prometheus | `render_prometheus_metrics` が文字列を返し、試験が一部の値を確認する | HTTP exporter／`127.0.0.1:9100/metrics`、p99<25msの実測、旧設計の全メトリクスはない |
| XDP | C／RustソースはIPv4送信元をキーにしたHASH map。ソースの上限は4096。旧版の隔離vethでPASS→DROP→PASSの実測記録がある | IPv6・CIDR／LPM、宛先サービス単位のscope、kernel TTLは公開版の機能ではない。配布objectと両ソースの再build一致、全NIC適合、CPU約0%・100%防御は保証しない |
| XDPの成否表示 | bridgeの書込みエラーを返す経路と `last_apply_error` がある | bridgeなしでも論理状態を進め得る。解除時のmap削除エラーを捨てる経路もある。`last_applied`／画面表示だけで実kernelの反映を断言しない |
| LSM・秘密消去 | Rust eBPFソースにLSM hook、loaderに任意のattach経路、Mock／Aya bridgeにダミーbuffer消去がある | 同梱C版objectのLSM運用、実アプリ秘密鍵との統合、PHP保護、ホスト全体の機密漏洩防止は未実証。ダミーbuffer消去は安全な全コピー消去の証拠ではない |
| WAL・再起動 | 保存・復元関数と、明示保存して別の状態機械へ読込むテストがある | 全署名応答前の永続化、電源断・disk failure・期限を延ばさない復元を一括保証しない。通常受信経路ではWAL保存を伴わず応答する箇所がある |
| ガード | defaultで `127.0.0.1` と `0.0.0.0`、追加exact IPv4、batch数・TTL・term／index境界の検査がある | 全loopback／管理網／peer／gatewayの自動保護、CIDR交差検査、検知の正当性の保証ではない |
| TUI・GIF | 同一プロセス内の4状態機械、デモ鍵、偽造RPC、試験が起動したprocessの停止。条件を満たすLinuxではveth XDPへ接続する経路がある | 独立4ホストの実証、自動攻撃検知、任意の攻撃processの安全な特定・停止ではない。UDP送出は約1ms待機の実装上の目安で、1000 pkt/sの実測保証ではない |
| 品質チェック・CI | lint・Clippy・testのworkflow／scriptがある | `proof_7` はobject／veth欠落やattach失敗で `Ok(())` を返す。green表示や `arena-smoke.sh` の固定PASS要約から実kernel実行・全環境適合を推定しない。`check.sh` はShellCheckが見つからない場合にその検査を実行しない |

## 根拠となる公開コード

- [RPC・QCの型](../veil-node/src/domain/entities.rs#L138)
- [ガードの実装](../veil-node/src/domain/policy.rs#L24)
- [通常daemon](../veil-node/src/main.rs#L106)
- [受信経路](../veil-node/src/infra/quic_mtls.rs#L271)
- [TTL・bypass・metrics](../veil-node/src/usecase/consensus_engine.rs#L1070)
- [WAL保存・復元](../veil-node/src/usecase/consensus_engine.rs#L1167)
- [公開map ABI](../veil-common/src/lib.rs#L8)
- [C版XDP](../veil-ebpf/src/veil_xdp_kern.c#L89)
- [Rust版LSM hook](../veil-ebpf/src/main.rs#L98)
- [kernel bridge](../veil-node/src/ebpf_bridge.rs#L66)
- [同一プロセスのQUIC試験](../veil-node/src/proof_tests.rs#L92)
- [kernel試験の成功扱いskip](../veil-node/src/proof_tests.rs#L415)
- [timeout明示駆動の試験](../veil-node/src/proof_tests.rs#L1395)
- [明示時計・bypassの試験](../veil-node/src/proof_tests.rs#L1467)
- [状態機械の複合試験](../veil-node/src/proof_tests.rs#L1531)
- [TUI mock process](../veil-node/src/tui.rs#L53)
- [固定PASS要約](../scripts/arena-smoke.sh#L21)
- [品質チェックの条件分岐](../scripts/check.sh#L15)

## クラウド・外部連携

- AWS：後続ローカル版の隔離XDPは合格。試したENA／MTU 9001では実NIC装着失敗。EKS・他構成は未検証。
- GCP：後続ローカル版の隔離XDPは合格。試したVirtIO-Net／MTU 1460では実NIC装着失敗。gVNIC・GKE・他構成は未検証。
- Azure：直接導入未検証。
- Cloudflare／Vercel：連携実装・受入れ記録未確認。旧案や環境の読取りは連携成功の証拠ではない。
- Warden／Blackhole／veil-rs／Arena：旧図は役割分担の構想。本リポジトリとの連携による一連の検知→合意→執行→監査の実証は確認できない。他リポジトリ単体の成果や保証範囲も本作へ移転しない。
- Nix MicroVM：設定と起動例はあるが、公開mainの独立2VMによる防御・停止復旧・運用受入れの成立は今回確認していない。

詳しい試験条件・限界は[クラウド試験要約](review/2026-10-27-CLOUD-VALIDATION-SUMMARY.md)を参照してください。「未検証」「試した構成で失敗」「ソースに存在」「運用受入れ未達」を区別します。

## 旧設計・履歴の訂正

[旧ロードマップ](V1_ARCHITECTURE_AND_ROADMAP.md)の2段階QC、LPM／IPv6、kernel TTL、50ms batching、Unix socket、exporter、復旧CLIは、公開実装の完成した機能一覧ではありません。5〜15ms、p99<25ms、3秒復旧、秒間2560件は実測保証として撤回しています。50ms待機を含む遅延に対するp99<25msという旧記載も整合しません。

2026-10-27のGitHub Release読取りでは、公開Releaseはv0.1.0／v0.1.1／v0.2.0でした。Cargo／変更履歴にあるv0.9.1は、同名Releaseの公開や旧計画の全達成を意味しません。

[初期レビューと改訂案](FEASIBILITY_REVIEW_AND_V1_DESIGN.md)の第1〜11節は旧対象版の所見です。第12節も未完の設計候補であり、「数学的に完全閉鎖」「時刻不確実でも安全に適用」といった結論を確定事項として扱いません。[CHANGELOG](../CHANGELOG.md)は当時の変更記録であり、機能名・版番号・成功件数は製品の運用保証ではありません。

## 今回確認した公開文書と限界

日本語・英語README、SECURITY、CONTRIBUTING、CHANGELOG、旧クラウド案、検証テスト解説、旧ロードマップ、初期レビュー／改訂案、再現記録、クラウド要約と、READMEのSVG説明図を確認しました。リンク先の公開ソース・シェル・CI設定を照合し、文書内のローカルリンクと見出し参照も検査しました。

外部のCloud Run、Cloudflare、Linux LPM、nftables、Raft論文、HotStuff-2著者解説は、引用している仕様・論点の参照先を確認しました。それぞれの一般仕様は本実装の成功を示すものではありません。外部製品の全機能・全履歴、非公開原ログ、記事本文そのものの監査は対象に含めていません。

今回の訂正で本番受入れやBFT保証が成立したわけではありません。開発・保守・追加課金試験の終了判断は継続します。
