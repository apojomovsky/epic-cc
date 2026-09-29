#!/usr/bin/env bash
# Print this checkout's content-addressed dev image tag.
# One definition for every caller (Makefile, hooks, helper scripts):
# the tag carries the Dockerfile plus uid/gid hash, so two worktrees
# with different Dockerfiles never share a tag. Byte-identical hash
# to epic-benchmarks report.sh; keep the pipeline in sync with it.
set -uo pipefail

root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
hash=$({ cat "$root/Dockerfile"; id -u; id -g; } | md5sum | cut -d' ' -f1)
test -n "$hash" || { echo "dev-image-tag: failed to hash the Dockerfile" >&2; exit 1; }
printf 'epic-cc-dev:local-%s\n' "$hash"
