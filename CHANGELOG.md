# 変更履歴 (Changelog)

All notable changes to this project will be documented in this file.

## [0.9.1] - 2026-10-23

### Added

- **Adversarial Chaos & Recovery Verification Suite (`proof_14`)**:
  - 4ノードクラスタにおける「ビザンチン偽造署名 Flood」「リーダー孤立・検閲」「ノード再起動 + Crash-Recovery WAL 復元」「`TimeoutCertificate` (TC) による View-Change」「旧リーダー復帰後の完全状態収束」を一気通貫で検証する統合カオステスト (`proof_14_v0_9_1_adversarial_chaos_partition_byzantine_flood_and_wal_recovery`) を追加。全23テスト（`proof_1`〜`proof_14` + R1〜R10 回帰テスト群）が 100% パスすることを実証。

## [0.5.0] - 2026-10-23

### Added

- **TTL 自動失効 (Relative TTL Auto-Expiry) とマイクロバッチ合意 (`SetPeerTrustBatch`)**:
  - `PeerTrustRule { peer_ip, trusted, ttl_secs }` および `MeshCommand::SetPeerTrustBatch`（最大 128 ルール一括合意）を追加。
  - 単調増加クロック (`monotonic_now_ns`) に基づく自動失効 (`advance_monotonic_clock` / `expire_stale_policies`) により、NTP 時刻ズレに影響されず期限切れルールを自動解除しカーネル eBPF マップからも回収（`proof_13`）。
- **緊急ローカルバイパス (`set_emergency_local_bypass`) & Prometheus メトリクス出力 (`render_prometheus_metrics`)**:
  - 誤検知時の緊急退避用にコンセンサス履歴を保持したままローカル XDP 判定を `Pass` に切り替えるバイパススイッチと、Prometheus 互換メトリクス出力機能を追加（`proof_13`）。

## [0.4.0] - 2026-10-23

### Added

- **BFT View-Change (`TimeoutVote` / `TimeoutCertificate`) と決定論的ラウンドロビン・リーダー交代**:
  - 悪意あるリーダーの沈黙・検閲攻撃（Liveness 妨害）に対処するため、$\lfloor 2N/3 \rfloor + 1$ 台の署名付き `TimeoutVote` から `TimeoutCertificate` (TC) を形成し、次の Term の決定論的ラウンドロビン・リーダー (`deterministic_leader_for_term`) へ強制交代する View-Change 機構を実装（`proof_12`）。

## [0.3.0] - 2026-10-23

### Security & Fixed

- **R1 (Prefix Hash Chain `digest_with_parent` & `entry_hash_at`)**:
  - `LogEntry::digest_with_parent` と $O(1)$ キャッシュ `chain_hashes` を導入し、各ログ Index の QC ハッシュが `[1..=index]` の全プレフィックス履歴を暗号学的に束縛するよう修正。末尾 `NoOp` の QC を悪用した異なる遮断命令の確定攻撃を阻止（回帰テスト `regression_r1_tail_qc_rejects_conflicting_prefixes`）。
- **R2 (Voted-Entry Lock & 二重署名防止 `voted_entry_hashes`)**:
  - 一度受理・署名した未コミットエントリを別 Term の競合値で上書きする際、新ブランチの有効な $\lfloor 2N/3 \rfloor + 1$ `commit_qc` を必須化。また同一 `(term, index)` に対する異なるハッシュへの二重署名を恒久拒否し、Term サンドイッチ攻撃および隠蔽 QC による別 Term 競合確定を阻止（回帰テスト `regression_r2_hidden_qc_rejects_conflicting_commits_across_terms`, `regression_r2_term_sandwich_prevents_double_voting_on_same_slot`）。
- **R3 (独立 OS プロセス間での共有 Root CA PKI・公開鍵自動同期・WAL 復元)**:
  - `ClusterMtlsPki::load_or_generate_dir` と `--cluster-dir` による共通 Root CA・ピア公開鍵束・Crash-Recovery WAL (`save_wal` / `load_wal`) の自動共有を追加（回帰テスト `regression_r3_multi_process_shared_ca_pki_handshakes_over_mtls`, `regression_wal_crash_recovery_prevents_double_vote_and_restores_ebpf_state`）。
- **R4 (未コミット・未適用時の厳格なエラー応答)**:
  - `MeshNode::propose_and_commit` で定足数コミットおよびカーネル eBPF 適用が確認できない場合は `Err` を返し、`ConsensusState::apply_committed_entries` でブリッジ書込み失敗時に `last_applied` を進めず `last_apply_error` を記録するよう修正（回帰テスト `regression_r4_apply_error_is_not_reported_as_applied`, `regression_r4_propose_and_commit_fails_when_peers_unreachable`）。
- **R5 / R6 / R7 (Linux `proof_7` ビルド修正・C 版 XDP の送信元ポート 22/10023 バイパス除去・マップ容量 4096 拡張)**:
  - `veil_xdp_kern.c` から `sport == 22 || sport == 10023` の無条件通過ルールを削除して `assets/veil_xdp_kern.o` を再コンパイルし、遮断解除時のマップ要素削除と `max_entries = 4096` 拡張を実施（回帰テスト `regression_r6_r7_linux_xdp_source_port_no_bypass_and_map_capacity`）。
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
  - ノード名から決定論的に秘密鍵を導出していた `derive_default_node_seed` を廃止し、`ring::rand::SystemRandom` による乱数生成と `--key-file` 永続化に統一。
  - `term >= MAX_SAFE_TERM` の境界オフバイワン修正と回帰テスト (`proof_11`) を追加。
  - 全ドキュメント（`README.md`, `SECURITY.md`, `docs/CONSENSUS_AND_EBPF_PROOF.md`, `docs/CLOUD_DEPLOYMENT_GUIDE.md`）から誇張表現を排除し、検証済み範囲と既知の限界を明記。

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
