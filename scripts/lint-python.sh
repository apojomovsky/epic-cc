#!/usr/bin/env bash
# python lint gate: ruff check plus ruff format --check over the repo's
# python files. One definition for every caller (pre-commit hook,
# pre-pr-check ritual, CI); ruff comes from the Dockerfile's pinned layer.
#
# Usage: bash scripts/lint-python.sh [--fix] [paths...] (default: repo
# root). --fix repairs in place. --no-cache avoids .ruff_cache writes.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fix=0
if [ "${1:-}" = "--fix" ]; then
  fix=1
  shift
fi

if [ "$#" -eq 0 ]; then
  set -- .
fi

if [ "$fix" -eq 1 ]; then
  ruff check --fix --no-cache "$@" && ruff format --no-cache "$@"
else
  ruff check --no-cache "$@" && ruff format --check --no-cache "$@"
fi
