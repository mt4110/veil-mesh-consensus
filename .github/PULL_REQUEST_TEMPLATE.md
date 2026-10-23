## 概要 (Summary)

<!-- 何を変更したか、なぜ変更したかを簡潔に記述してください -->

## 検証内容 (Verification)

- [ ] `bash scripts/lint-markdown.sh`（Markdown リントがすべてパスすること）
- [ ] `bash scripts/check.sh`（`cargo fmt`, `cargo clippy`, `shellcheck`, 全証明テストがパスすること）
- [ ] （実機 eBPF 変更時）`bash scripts/deploy-pi-node.sh test` での確認
