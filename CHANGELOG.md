# 変更履歴 (Changelog)

2026-10-27に開発終了。本書は当時の変更記録です。版番号・機能名・当時の試験報告は、本番受入れや一般的な安全性の保証ではありません。現在の記述範囲は[確認状況](docs/VALIDATION_STATUS.md)と[試験の制約](docs/CONSENSUS_AND_EBPF_PROOF.md)を参照してください。

## [0.9.1] - 2026-10-23

### Added

- **Adversarial Chaos & Recovery Verification Suite (`proof_14`)**:
  - 同一プロセス内の4状態機械で「ビザンチン偽造署名 Flood」「リーダー孤立・検閲」「ノード再起動 + Crash-Recovery WAL 復元」「`TimeoutCertificate` (TC) による View-Change」「旧リーダー復帰を模した状態の一致」を検査する複合シナリオ試験 (`proof_14_v0_9_1_adversarial_chaos_partition_byzantine_flood_and_wal_recovery`) を追加。当時の合格報告は限定シナリオのもの。RPC配送・timeout・WAL保存／復元を試験内で明示駆動し、実hostのpacket loss・電源断・全BFTシナリオを実証したものではない。今回、その当時の全実行ログや製品試験の再実行は確認していない。

## [0.5.0] - 2026-10-23

### Added

- **TTL 自動失効 (Relative TTL Auto-Expiry) とマイクロバッチ合意 (`SetPeerTrustBatch`)**:
  - `PeerTrustRule { peer_ip, trusted, ttl_secs }` および `MeshCommand::SetPeerTrustBatch`（最大 128 ルール一括合意）を追加。
  - ユーザー空間の `monotonic_now_ns` と `advance_monotonic_clock` / `expire_stale_policies` を追加（`proof_13` は時計を明示駆動）。kernel自律失効、通常daemonの実時計駆動、NTP異常・再起動を通じた期限保証は未実証。batchコマンドの存在は50ms入力集約の実装を意味しない。
- **緊急ローカルバイパス (`set_emergency_local_bypass`) & Prometheus メトリクス出力 (`render_prometheus_metrics`)**:
  - ユーザー空間の `check_xdp_packet` をPassにするflagと、Prometheus形式の文字列生成を追加（`proof_13`）。kernel link／mapのbypass操作、管理CLI、HTTP exporterは追加していない。

## [0.4.0] - 2026-10-23

### Added

- **BFT View-Change (`TimeoutVote` / `TimeoutCertificate`) と決定論的ラウンドロビン・リーダー交代**:
  - 悪意あるリーダーの沈黙・検閲攻撃（Liveness 妨害）に対処するため、$\lfloor 2N/3 \rfloor + 1$ 台の署名付き `TimeoutVote` から `TimeoutCertificate` (TC) を形成し、次の Term の決定論的ラウンドロビン・リーダー (`deterministic_leader_for_term`) へ交代させる状態機械の処理を追加（`proof_12`）。試験はtimeoutを直接呼び、通常daemonでの自動timer・検閲回復を受入れたものではない。

## [0.3.0] - 2026-10-23

### Security & Fixed

- **R1 (Prefix Hash Chain `digest_with_parent` & `entry_hash_at`)**:
  - `LogEntry::digest_with_parent` と $O(1)$ キャッシュ `chain_hashes` を導入し、各ログ Index の QC ハッシュが `[1..=index]` の全プレフィックス履歴を暗号学的に束縛するよう修正。末尾 `NoOp` の QC を悪用する特定のprefix改ざんcaseを拒否する処理を追加（回帰テスト `regression_r1_tail_qc_rejects_conflicting_prefixes`）。
- **R2 (Voted-Entry Lock & 二重署名防止 `voted_entry_hashes`)**:
  - 一度受理・署名した未コミットエントリを別 Term の競合値で上書きする際、新ブランチの有効な $\lfloor 2N/3 \rfloor + 1$ `commit_qc` を必須化。また同一 `(term, index)` に対する異なるハッシュへの同じ状態機械が保持する投票slotで二重署名を拒否する処理を追加し、Term サンドイッチ攻撃および隠蔽 QC による別 Term 競合確定の特定caseを扱う（回帰テスト `regression_r2_hidden_qc_rejects_conflicting_commits_across_terms`, `regression_r2_term_sandwich_prevents_double_voting_on_same_slot`）。
- **R3 (独立 OS プロセス間での共有 Root CA PKI・公開鍵自動同期・WAL 復元)**:
  - `ClusterMtlsPki::load_or_generate_dir` と `--cluster-dir` による共有CA・公開鍵探索、node別WAL保存／復元を追加。PKIの回帰caseは同一プロセスで別PKI objectを作るもので、独立OSプロセスのクラスタ全体の受入れではない。WALの明示保存／復元caseも、全受信応答前の永続化を証明しない（`regression_r3_multi_process_shared_ca_pki_handshakes_over_mtls`, `regression_wal_crash_recovery_prevents_double_vote_and_restores_ebpf_state`）。
- **R4 (未コミット・未適用時のエラー応答処理)**:
  - `MeshNode::propose_and_commit` が `commit_index` と `last_applied` を確認して不足時に `Err` を返すよう変更し、`ConsensusState::apply_committed_entries` でブリッジ書込み失敗時に `last_applied` を進めず `last_apply_error` を記録するよう修正（`regression_r4_apply_error_is_not_reported_as_applied`, `regression_r4_propose_and_commit_fails_when_peers_unreachable`）。bridgeを接続していない場合や、map削除エラーを捨てる経路もあるため、これだけで実kernel適用の確認済みとはしない。
- **R5 / R6 / R7 (Linux `proof_7` ビルド修正・C 版 XDP の送信元ポート 22/10023 バイパス除去・マップ容量 4096 拡張)**:
  - Cソースのport bypassを除き、ソースのmap上限を4096へ変更、bridgeに解除時のmap削除処理を追加した。当時のobject再build記載は履歴であり、今回C／Rustソースと同梱objectの再現build一致を検証していない。Linux回帰caseの存在だけで実kernel実行済みとはしない（`regression_r6_r7_linux_xdp_source_port_no_bypass_and_map_capacity`）。
- **R8 / R9 / R10 (`0600` 鍵パーミッション強制・1024件チャンク送信・Term 上限選挙保護)**:
  - 既存の `0644` 鍵やシンボリックリンクを `PermissionDenied` で拒否し、新規生成時は `OpenOptionsExt::mode(0o600)` で排他作成（`regression_r8`）。1025件以上の Catch-up 送信を `MAX_LOG_ENTRIES_PER_RPC` (1024件) 以下に分割（`regression_r9`）。`MAX_SAFE_TERM - 1` での選挙 Term 単調増加と上限到達時の二重投票防止を実装（`regression_r10`）。

## [0.2.0] - 2026-10-23

### Changed

- **クリーンアーキテクチャへの構造刷新 (`veil-node/src/{domain,usecase,adapter,infra}`)**:
  - `domain`: エンティティ定義と保護対象 IP 誤遮断防止ガードレール (`SecurityPolicyGuard`)・境界値検証ロジックを分離。
  - `usecase`: `ConsensusState` ステートマシンと `EbpfBridge` 出力ポートを分離。
  - `adapter`: `ring` による Ed25519 CSPRNG 乱数鍵生成・`0600` 鍵ファイル永続化 (`NodeCryptoIdentity`) とワイヤーフレーム (`WireMessage`) を分離。
  - `infra`: `quinn` mTLS 1.3 通信 (`ClusterMtlsPki`)、Aya カーネルブリッジ、Ratatui TUI アリーナを分離。
- **CSPRNG 乱数鍵生成と `--key-file` / 複数 `--peer` 対応**:
  - ノード名から決定論的に秘密鍵を導出していた `derive_default_node_seed` を廃止し、通常daemonに `ring::rand::SystemRandom` による乱数生成と `--key-file` 永続化を導入。TUIは既知のデモseedを使う別の試験経路。
  - `term >= MAX_SAFE_TERM` の境界オフバイワン修正と回帰テスト (`proof_11`) を追加。
  - 当時、README・SECURITY・試験解説・クラウド案を更新した。後に残存した誇張・実装と構想の混同が判明したため、この変更を「全主張の正確性確認済み」の根拠にはしない。

## [0.1.1] - 2026-10-23

### Fixed

- **QUIC ネットワーク経路での `SignedConsensusRpc` 強制と mTLS 化**:
  - `WireMessage::Consensus` を `SignedConsensusRpc` のみに変更し、受信時に `handle_signed_rpc` を強制（回帰テスト `proof_9`）。
  - `SkipServerVerification` を廃止し、`ClusterMtlsPki` による相互 TLS 認証を導入。
- **コミット進行時のマルチシグ `QuorumCertificate` 必須化と 4ノード定足数 ($\lfloor 2N/3 \rfloor + 1$)**:
  - `commit_qc: None` でのコミット要求を拒否し、`AppendEntriesResponse` の `entry_hash` と `qc_vote_signature` をリーダーが検証するよう修正（回帰テスト `proof_8`, `proof_10`）。
- **境界値検証と導入ガイド追加**:
  - `index == 0` のアンダーフロー Panic 防止（`proof_11`）。
  - `docs/CLOUD_DEPLOYMENT_GUIDE.md`、`.pre-commit-config.yaml`、`markdownlint-cli2` 検査スクリプトを追加。

## [0.1.0] - 2026-10-23

### Added

- 初回実験プロトタイプ公開（QUIC + Raft ベース合意状態機械、Linux XDP `veil_xdp_kern.c`、Ratatui TUI アリーナ、Nix Flakes MicroVM 構成）。
