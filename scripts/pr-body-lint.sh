#!/usr/bin/env bash
# pr-body-lint.sh — fail when a PR body contains literal \n escapes.
# Resilience for oh-my-pi shell-quoting bug (epic-cc#129): `gh pr create --body "a\nb"`
# never expands, GitHub renders literal \n as text.
# Single source is epic_tasks/gh.py:contains_literal_escapes / normalize_pr_body
# and epic_tasks/takeoff.py:check_pr_body; this shell is a thin wrapper
# so all three repos share one rule. Fallback grep is fixed-string, not ANSI-C.
# Usage:
#   bash scripts/pr-body-lint.sh [file]          # lint a file (or stdin with -)
#   bash scripts/pr-body-lint.sh --pr            # lint `gh pr view --json body` for current branch
#   python3 -c 'from epic_tasks.gh import normalize_pr_body; ...' to heal.
set -euo pipefail

lint_file() {
  local file="$1"
  # Prefer single-source python helper; fallback to fixed-string grep.
  if python3 -c "from epic_tasks.gh import contains_literal_escapes" 2>/dev/null; then
    if python3 -c "import sys; from epic_tasks.gh import contains_literal_escapes; sys.exit(0 if contains_literal_escapes(open(sys.argv[1], encoding='utf-8', errors='ignore').read()) else 1)" "$file"; then
      echo "pr-body-lint: literal \\n/\\r in $file — use real newlines (gh pr create --body-file - <<'EOF')" >&2
      echo "hint: cat <<'EOF' > /tmp/pr_body.md; gh pr create --body-file /tmp/pr_body.md" >&2
      python3 -c "import sys; data=open(sys.argv[1], encoding='utf-8', errors='ignore').read(); print(data[:400].replace(chr(10), '↵'))" "$file" >&2 || true
      exit 1
    fi
  else
    if grep -qF '\n' "$file" || grep -qF '\r' "$file"; then
      echo "pr-body-lint: literal \\n/\\r in $file — use real newlines (gh pr create --body-file - <<'EOF')" >&2
      echo "hint: cat <<'EOF' > /tmp/pr_body.md; gh pr create --body-file /tmp/pr_body.md" >&2
      grep -nF '\n' "$file" | head -n 20 >&2 || true
      exit 1
    fi
  fi
  echo "pr-body-lint: $file ok"
}

if [[ "${1:-}" == "--pr" ]]; then
  if ! command -v gh >/dev/null 2>&1; then
    echo "pr-body-lint: gh not found — skipped" >&2; exit 0
  fi
  body=$(gh pr view --json body --jq .body 2>&1 || true)
  if echo "$body" | grep -q "no pull" || echo "$body" | grep -qi "could not find"; then
    echo "pr-body-lint: no PR for branch — skipped"; exit 0
  fi
  tmp=$(mktemp)
  printf "%s" "$body" > "$tmp"
  lint_file "$tmp"
  rm -f "$tmp"
  exit 0
fi

if [[ $# -eq 0 ]]; then
  echo "usage: $0 [--pr] [file|-]" >&2; exit 2
fi
for f in "$@"; do
  if [[ "$f" == "-" ]]; then
    tmp=$(mktemp); cat > "$tmp"; lint_file "$tmp"; rm -f "$tmp"
  else
    lint_file "$f"
  fi
done
