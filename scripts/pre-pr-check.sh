#!/usr/bin/env bash
# Unified takeoff: wraps `epic-tasks takeoff` (canonical checks) and runs
# this repo's python lint gate first (ruff, pinned in the Dockerfile).
# Preserves `make pre-pr-check` as the entry point and forwards flags.
#
# Usage: bash scripts/pre-pr-check.sh [--test] [--base <ref>]
#   --test   also run the full suite
#   --base   base ref to diff against (default origin/master or $BASE_REF)
set -uo pipefail

# python lint first: ruff lives in the dev image, same as every other
# check here. Skipped only when the image is not built yet, the same
# leniency as the pre-commit hook. Mount the repo root, not $PWD, so this
# works from any subdirectory.
root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
if docker image inspect epic-cc-dev:local >/dev/null 2>&1; then
  docker run --rm \
    -v "$root":/workspace -w /workspace epic-cc-dev:local \
    bash scripts/lint-python.sh || exit 1
fi

# Prefer a PATH install, fall back to sibling checkout layout.
if command -v epic-tasks >/dev/null 2>&1; then
  exec epic-tasks takeoff "$@"
fi

# Try sibling layout: EPIC_TASKS_ROOT or parent of this repo's parent
for cand in "${EPIC_TASKS_ROOT:-}" "$(git rev-parse --show-toplevel 2>/dev/null)/../epic-tasks" "$HOME/projects/epic-tasks"; do
  [ -z "$cand" ] && continue
  cand=$(realpath -m "$cand" 2>/dev/null || echo "$cand")
  if [ -x "$cand/epic-tasks" ]; then
    exec "$cand/epic-tasks" takeoff "$@"
  fi
done

echo "epic-tasks not found — install it:" >&2
echo "  ln -s \"\$PWD/epic-tasks/epic-tasks\" ~/.local/bin/epic-tasks" >&2
echo "  gh auth refresh -s project" >&2
exit 4
