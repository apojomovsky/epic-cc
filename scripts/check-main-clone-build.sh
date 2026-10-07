#!/usr/bin/env bash
# Refuse container builds from the main clone while agents are active.
# Every worktree mounts at /workspace, and the target cache is keyed by
# host CURDIR, so two agents building from the same directory (the usual
# cause: running make in the main clone instead of a worktree) share one
# cache and corrupt each other's builds. Work from .worktrees/<name>.
#
# Usage: bash scripts/check-main-clone-build.sh
#   EPIC_ALLOW_MAIN_CLONE_BUILD=1 overrides the refusal.
set -uo pipefail

[ "${EPIC_ALLOW_MAIN_CLONE_BUILD:-0}" = "1" ] && exit 0
[ -d .worktrees ] || exit 0
[ -n "$(ls -A .worktrees 2>/dev/null)" ] || exit 0

units=$(systemctl --user list-units 'epic-agent-*' --state running --no-legend 2>/dev/null || true)
if [ -z "$units" ]; then
  echo "check-main-clone-build: worktrees exist but no epic-agent-* units are running, proceeding" >&2
  exit 0
fi

echo "check-main-clone-build: refusing build from the main clone while agents are active" >&2
echo "run make from your worktree (.worktrees/<name>), or set EPIC_ALLOW_MAIN_CLONE_BUILD=1" >&2
exit 1
