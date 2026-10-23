# 貢献ガイド (Contributing Guide)

[README（日本語）](README.md) · [English README](README_EN.md) · [セキュリティ方針](SECURITY.md) · [クラウド導入ガイド](docs/CLOUD_DEPLOYMENT_GUIDE.md)

`veil-mesh-consensus` への Issue 報告や Pull Request を歓迎します。変更を提案する際は、以下の開発・品質基準を確認してください。

## 1. 変更前の確認事項

1. **不変条件（Invariants）の維持**:
   - 本プロジェクトは 8 つの証明テスト (`proof_1` 〜 `proof_8`) によって、単一リーダー選出、QUIC 越しの XDP 即時遮断、Split-Brain 耐性、LSM ステルス GC ゼロ消去、Ed25519 ビザンチン耐性、およびマルチシグ `QuorumCertificate` によるリーダー二枚舌防止を保証しています。これらを弱める変更は行わないでください。
2. **機密情報の非混入**:
   - 実環境の秘密鍵、API トークン、実サービスの生トラフィックログをコミットや Issue に含めないでください。

## 2. ローカル品質チェックと Pre-commit フック

コミット前に、以下のスクリプトで **Markdown リント（`.gitignore` 対象外の全 `.md` ファイル）**、**ShellCheck**、**`cargo fmt` / `clippy`**、および **全証明テスト** がすべてパスすることを確認してください。

```bash
# 全チェックの一括実行
bash scripts/check.sh

# Markdown リント単体の実行
bash scripts/lint-markdown.sh

# pre-commit フックの有効化（任意）
pre-commit install
```
