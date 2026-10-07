#!/usr/bin/env bash
# Serialize builds that share one target cache dir. Hosts without
# flock run unlocked rather than failing every build.
#
# Usage: bash scripts/with-target-lock.sh <lockfile> <cmd> [args...]
set -uo pipefail

lock=$1
shift
mkdir -p "$(dirname "$lock")"
if command -v flock >/dev/null 2>&1; then
  exec flock "$lock" "$@"
fi
echo "with-target-lock: flock not found, running $* unlocked" >&2
exec "$@"
