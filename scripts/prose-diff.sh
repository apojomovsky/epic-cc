#!/usr/bin/env bash
# Diff-scoped prose for the takeoff ritual's comment review: a thin
# wrapper over `epic-tasks prose`, so the lint rules live in one
# place and every epic repo runs the same lint. Prints every added
# comment block in BASE_REF..HEAD; `--verify` exits 1 with the fix
# list when a block violates the mechanical rules.
#
# Usage: bash scripts/prose-diff.sh [--verify]
#   BASE_REF overrides the base branch (forks: BASE_REF=<fork>/master)

set -uo pipefail

export BASE_REF="${BASE_REF:-origin/master}"

if command -v epic-tasks >/dev/null 2>&1; then
  exec epic-tasks prose "$@"
fi

# Try sibling layout: EPIC_TASKS_ROOT or parent of this repo's parent
for cand in "${EPIC_TASKS_ROOT:-}" "$(git rev-parse --show-toplevel 2>/dev/null)/../epic-tasks" "$HOME/projects/epic-tasks"; do
  [ -z "$cand" ] && continue
  cand=$(realpath -m "$cand" 2>/dev/null || echo "$cand")
  if [ -x "$cand/epic-tasks" ]; then
    exec "$cand/epic-tasks" prose "$@"
  fi
done

echo "epic-tasks not found, install it:" >&2
echo "  ln -s \"\$PWD/epic-tasks/epic-tasks\" ~/.local/bin/epic-tasks" >&2
echo "  gh auth refresh -s project" >&2
exit 4
