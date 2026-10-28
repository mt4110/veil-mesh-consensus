Archived local working document / ローカル最終文書の保全。

2026-10-28: Preserved from the local changes involved in the stash conflict. Historical statements, next steps, and relative links below retain their original context; they are not current recommendations or authorization to resume development. See the repository root README and docs/PROJECT_CLOSURE.md for closure status.

---

# セキュリティ方針 (Security Policy)

> 現在の試験導入対象は別binaryのローカル一時BANです。[現在の契約・限界](../../../README.md)と[導入前チェック](../../local-ban/PILOT_CHECKLIST.md)を参照してください。以下の第1節は既存BFT実験の記載時点の説明であり、一時BANに定足数・署名・WALの保証を与えるものではありません。第2節の非公開情報の扱いと報告方針は共通です。

[README（日本語）](../../../README.md) · [English README](../../../README_EN.md) · [クラウド導入ガイド](../../CLOUD_DEPLOYMENT_GUIDE.md)

## 1. 対象範囲と脅威モデル

`veil-mesh-consensus` は、mTLS QUIC (`Tokio` + `quinn`) トランスポート上の署名付き Raft 系合意形成（Ed25519 `SignedConsensusRpc` + SHA-256 エントリダイジェスト + マルチシグ `QuorumCertificate`）で決定した遮断ルールを、Linux カーネル空間の eBPF (`Aya` XDP パケットフィルタ) およびユーザー空間 LSM ブリッジへ反映する実験的プロトタイプです。

### 保護・検証の対象（テストスイートで確認済みの範囲）

- **スプリットブレイン（誤遮断）の抑制**: 単一ノードの誤検知やネットワーク分断時において、少数派パーティションのノードが独断で eBPF 遮断ポリシーを更新しないことを検証しています（`proof_3`）。
- **署名検証とマルチシグ QC による不正 RPC・単独コミットの拒否**:
  - 外部からのなりすまし RPC や途中経路でのペイロード改ざんを Ed25519 署名検証 (`handle_signed_rpc`) で棄却します（`proof_5`, `proof_9`）。
  - リーダー自身が侵害された場合の二枚舌（Equivocation）や `commit_qc: None` による単独コミット要求を、SHA-256 ダイジェストと $\lfloor 2N/3 \rfloor + 1$ マルチシグ `QuorumCertificate` で拒否します（`proof_8`, `proof_10`）。
  - 各ノードの秘密鍵は OS の暗号学的乱数 (`ring::rand::SystemRandom`) から生成し、公開情報（ノード名）から推測できない設計としています（`proof_11`）。
- **保護対象 IP ガードレール（自己窒息防止）**:
  - ループバック (`127.0.0.1`) や登録済みの保護対象 IP に対する遮断提案をドメイン層 (`SecurityPolicyGuard`) で拒否します（`proof_11`）。

### 既知の限界と注意事項（Known Limitations）

- **View-Change（リーダー沈黙時の自動交代）未実装**: 現在の合意層は Raft ベースであり、PBFT/HotStuff 系の View-Change タイマーは未実装です。
- **XDP と LSM の実装段階**: IPv4 の `XDP_DROP` (`veil_xdp_kern.c`) は実機 Linux カーネルで検証済みですが、LSM (`socket_connect`) 遮断と鍵ゼロ消去は現時点ではユーザー空間 (`MockEbpfBridge`) のシミュレーションです。また XDP は現在 IPv4 送信元 IP のみを対象としています。
- **ホスト `root` 侵害および定足数侵害**: すでに `root` を奪取されたノード内部での操作や、定足数（$\ge \lfloor 2N/3 \rfloor + 1$）の秘密鍵が同時に漏洩した場合の防御は対象外です。

## 2. 脆弱性・不具合の報告方法

機密情報や実サービスに対する攻撃手順を公開 Issue に記載しないでください。GitHub の Private Vulnerability Reporting（非公開脆弱性報告）が有効な場合はそちらから報告してください。非公開経路が利用できない場合は、秘密情報や攻撃コードを含まない概要のみを Issue で連絡してください。
