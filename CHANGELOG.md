# 変更履歴 (Changelog)

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added

- **クラウド・エッジ環境別 実践導入ガイド (`docs/CLOUD_DEPLOYMENT_GUIDE.md`)**: AWS (EC2/ENA)、GCP (GCE/gVNIC)、Microsoft Azure (Azure VM/NSG)、Cloudflare、Vercel それぞれの対応可否マトリクスとステップバイステップの導入手順書を追加。
- **品質保証・自動リント基盤**: `.pre-commit-config.yaml`、`.markdownlint-cli2.jsonc`、`scripts/lint-markdown.sh`（`.gitignore` 対象外の全 `.md` ファイルを自動検査）、`scripts/check.sh`、および GitHub Actions CI (`.github/workflows/ci.yml`) を追加。
- **OSS ガバナンス文書**: `SECURITY.md`、`CONTRIBUTING.md`、`CHANGELOG.md`、および Issue / PR テンプレート一式を追加。

## [0.1.0] - 2026-10-23

### Added

- **低レイヤー QUIC + BFT Raft コンセンサスエンジン (`veil-node/src/consensus.rs`)**:
  - `Tokio` + `quinn` (TLS 1.3 over UDP 5000) 双方向ストリーム上の合意形成。
  - Ed25519 署名検証 (`SignedConsensusRpc`) による外部なりすまし・改ざんの 100% 阻止。
  - SHA-256 ログダイジェストと $2f+1$ マルチシグ合意証明書 (`QuorumCertificate`) によるリーダー裏切り・二枚舌（Equivocation）の完全阻止。
- **Linux カーネル eBPF 即時遮断 & LSM ステルス GC (`veil-ebpf`, `veil-node/src/ebpf_bridge.rs`)**:
  - `PEER_POLICY_MAP` による NIC 着信直後の `XDP_DROP` パケット破棄。
  - `LSM_POLICY_MAP` による `socket_connect` の `-EPERM` 封鎖と機密鍵メモリの即時ゼロ消去 (`Zeroize`)。
- **インタラクティブ TUI アリーナ (`veil-node/src/tui.rs`) & 8大証明テスト (`proof_1` 〜 `proof_8`)**:
  - 実 OS モック攻撃プロセスの生成からコンセンサス合意、`sys_bpf` (`XDP_DROP`)、`sys_kill(PID, SIGKILL=9)` による撃退までを可視化する TUI ダッシュボードと実機 `pi-node` 検証スクリプト。
