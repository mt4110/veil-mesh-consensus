#!/usr/bin/env bash
# ==============================================================================
# veil-mesh-consensus: Full Repository Quality & Proof Check
# ==============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

echo "=== [1/4] Checking Markdown Lint (all non-gitignored .md files) ==="
bash scripts/lint-markdown.sh

echo "=== [2/4] Checking Shell Scripts with ShellCheck ==="
if command -v shellcheck >/dev/null 2>&1; then
  shellcheck scripts/*.sh
elif [ -x "/nix/store/avhj92vrg6c79avn13mk4xc4l1r2l73i-shellcheck-0.11.0-bin/bin/shellcheck" ]; then
  /nix/store/avhj92vrg6c79avn13mk4xc4l1r2l73i-shellcheck-0.11.0-bin/bin/shellcheck scripts/*.sh
fi

echo "=== [3/4] Running Cargo Format & Clippy Checks ==="
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

echo "=== [4/4] Running All Mathematical & Consensus Proof Tests ==="
cargo test --workspace
