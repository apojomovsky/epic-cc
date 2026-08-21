#!/usr/bin/env bash
# Diff-scoped prose surface for the takeoff ritual's comment/doc review
# step (AGENTS.md): every added comment block and markdown hunk from
# BASE_REF...HEAD, in full, so the agent doesn't have to re-derive the
# diff. It flags block length and hardcoded counts as hints only;
# guessing "AI leftover" phrasing is a losing, provider-specific bet
# across the models this repo sees, so content judgment stays the
# agent's job, every block, flagged or not.
#
# Usage: bash scripts/prose-diff.sh
#   BASE_REF overrides the base branch (forks: BASE_REF=<fork>/master)

set -uo pipefail

BASE_REF="${BASE_REF:-origin/master}"

comment_blocks=0
doc_files=0

marker_for_file() {
  case "$1" in
    *.rs) printf '%s' '^(///|//!|//)' ;;
    *.sh|*.toml|*.yml|*.yaml) printf '%s' '^#' ;;
    */Makefile|Makefile) printf '%s' '^#' ;;
    */Dockerfile*|Dockerfile*) printf '%s' '^#' ;;
    *) printf '' ;;
  esac
}

files=$(git diff --name-only --diff-filter=ACMR "$BASE_REF"...HEAD 2>/dev/null \
  | grep -vE '^(vendor/|target/|\.worktrees/|\.claude/worktrees/|docs/superpowers/plans/)' || true)

echo "== Comments added in $BASE_REF...HEAD =="
echo ""

for f in $files; do
  marker=$(marker_for_file "$f")
  [ -z "$marker" ] && continue
  [ -f "$f" ] || continue

  newline=0
  block_start=0
  block_content=""
  block_len=0

  flush() {
    if [ "$block_len" -gt 0 ]; then
      comment_blocks=$((comment_blocks + 1))
      end=$((block_start + block_len - 1))
      tag=""
      [ "$block_len" -gt 3 ] && tag="  [hint: >3 lines, confirm it's justified]"
      echo "  $f:$block_start-$end ($block_len line(s))$tag"
      printf '%s' "$block_content" | sed 's/^/    /'
      echo ""
    fi
    block_start=0
    block_content=""
    block_len=0
  }

  while IFS= read -r line; do
    case "$line" in
      @@*)
        flush
        newline=$(printf '%s' "$line" | sed -nE 's/^@@ -[0-9]+(,[0-9]+)? \+([0-9]+)(,[0-9]+)? @@.*/\2/p')
        ;;
      +++*) ;;
      +*)
        content="${line#+}"
        trimmed="${content#"${content%%[![:space:]]*}"}"
        if printf '%s' "$trimmed" | grep -qE "$marker"; then
          [ "$block_len" -eq 0 ] && block_start=$newline
          block_content="${block_content}${content}"$'\n'
          block_len=$((block_len + 1))
        else
          flush
        fi
        newline=$((newline + 1))
        ;;
      -*) ;;
      *) ;;
    esac
  done < <(git diff -U0 "$BASE_REF"...HEAD -- "$f" 2>/dev/null)
  flush
done

echo "== Markdown diffs added/modified in $BASE_REF...HEAD =="
echo ""

md_files=$(printf '%s\n' "$files" | grep -E '\.md$' || true)
for f in $md_files; do
  [ -f "$f" ] || continue
  doc_files=$((doc_files + 1))
  echo "  -- $f --"
  hunk=$(git diff "$BASE_REF"...HEAD -- "$f" 2>/dev/null)
  printf '%s\n' "$hunk" | sed 's/^/    /'
  if printf '%s' "$hunk" | grep -qE '^\+.*[0-9]+ +(tests?|files?|crates?|lines? of code)\b' \
     || printf '%s' "$hunk" | grep -qE '^\+.*(├──|└──)'; then
    echo "    [hint: possible coupling to a volatile fact (count/tree); confirm it won't go stale]"
  fi
  echo ""
done

echo "SUMMARY: comment_blocks=$comment_blocks doc_files=$doc_files"
exit 0
