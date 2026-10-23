#!/usr/bin/env bash
# ==============================================================================
# Lint all Git-tracked / non-gitignored Markdown (.md) files
# ==============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

# Resolve markdownlint-cli2 from PATH or local Nix store
if command -v markdownlint-cli2 >/dev/null 2>&1; then
  MDLINT="markdownlint-cli2"
elif [ -x "/nix/store/mrxkc3883d5hvpmhjb592q9nx8wfpl5x-markdownlint-cli2-0.23.3/bin/markdownlint-cli2" ]; then
  MDLINT="/nix/store/mrxkc3883d5hvpmhjb592q9nx8wfpl5x-markdownlint-cli2-0.23.3/bin/markdownlint-cli2"
elif [ -x "/nix/store/hcfqmshasya31pdsighyfar0hkwnspxh-markdownlint-cli2-0.21.0/bin/markdownlint-cli2" ]; then
  MDLINT="/nix/store/hcfqmshasya31pdsighyfar0hkwnspxh-markdownlint-cli2-0.21.0/bin/markdownlint-cli2"
else
  echo "Running markdownlint-cli2 via npx..."
  MDLINT="npx --yes markdownlint-cli2"
fi

# Collect all .md files that are tracked or not ignored by .gitignore
MD_FILES=()
while IFS= read -r file; do
  if [ -f "$file" ]; then
    MD_FILES+=("$file")
  fi
done < <(git ls-files --cached --others --exclude-standard "*.md" "**/*.md" | sort -u)

if [ "${#MD_FILES[@]}" -eq 0 ]; then
  echo "No Markdown files found to lint."
  exit 0
fi

echo "=== Linting ${#MD_FILES[@]} Markdown files (respecting .gitignore) ==="
printf '  - %s\n' "${MD_FILES[@]}"
$MDLINT "${MD_FILES[@]}"
echo "=== All Markdown files passed lint! ==="
