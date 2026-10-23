#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
RGC_DIR="/Users/takemuramasaki/_workspace/readme-gif-crafter"

cd "$PROJECT_ROOT"

if [ ! -f "assets/demo.mp4" ]; then
    echo "=================================================================="
    echo " [INFO] assets/demo.mp4 がまだ配置されていません。"
    echo " 以下のコマンドで TUI 自動デモを起動し、画面録画を assets/demo.mp4 に保存してください："
    echo ""
    echo "   cargo run -p veil-node -- --tui-demo"
    echo "   # または対話操作モード:"
    echo "   cargo run -p veil-node -- --tui"
    echo "=================================================================="
    exit 1
fi

echo "Building readme-gif-crafter (rgc)..."
RGC_BIN="$RGC_DIR/target/release/rgc"
if [ ! -x "$RGC_BIN" ]; then
    (cd "$RGC_DIR" && cargo build --release)
fi

echo "Generating assets/demo.gif and updating README.md & README_EN.md..."
"$RGC_BIN" assets/demo.mp4 --output assets/demo.gif --update-readme README.md --preset github
"$RGC_BIN" assets/demo.gif --output assets/demo.gif --markdown-only --update-readme README_EN.md

echo "Done! Updated assets/demo.gif, README.md, and README_EN.md."
