# 変更履歴 (Changelog)

All notable changes to this project will be documented in this file.

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
