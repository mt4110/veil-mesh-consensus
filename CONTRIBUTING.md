# 開発終了後の資料と過去の開発チェック

[README（日本語）](README.md) · [English README](README_EN.md) · [セキュリティ上の制約](SECURITY.md) · [確認状況](docs/VALIDATION_STATUS.md)

2026-10-27に開発を終了しました。リポジトリは研究記録としてアーカイブし、保守・機能追加・セキュリティ修正は予定していません。新規Issue／PRを継続受付する開発プロジェクトとして案内していません。

## 1. 記録を参照する際の注意

- `proof_*` は特定シナリオのテスト名であり、BFT・Split-Brain耐性・ゼロ漏洩などを一般に保証するものではありません。
- 状態機械、同一プロセス内QUIC、Mock、Linux隔離veth、実NIC、本番サイトの結果を分けてください。未実行・skipは実測合格に数えません。
- 初期レビュー、後続の公開コード、未公開ローカル版、設計候補を区別してください。旧ロードマップを自動的に開発再開計画としないでください。
- 秘密鍵・APIトークン・生トラフィック・非公開の接続先を公開資料へ含めないでください。

## 2. 過去のチェックscript

以下は開発当時の研究用scriptです。実行すれば現在の環境や実kernelの検証がすべて成立するという保証ではありません。

```bash
bash scripts/check.sh
bash scripts/lint-markdown.sh
```

`check.sh` はMarkdown lint、利用できる場合のShellCheck、Rust fmt／Clippy、workspace試験を実行します。ShellCheckが見つからない場合は検査を実行しません。`proof_7` はobject・veth不在やattach失敗を成功扱いで返すため、全体のexit 0を実kernel合格として使えません。[各試験の範囲](docs/CONSENSUS_AND_EBPF_PROOF.md)を確認してください。

`deploy-pi-node.sh` は専用の過去環境を前提にし、リモートのネットワークやtrace設定を変更し得ます。一般的な安全な導入・復旧ツールではありません。`arena-smoke.sh` の固定形式PASS要約も、実行されたcase・skip・kernel証拠の代わりにはなりません。
