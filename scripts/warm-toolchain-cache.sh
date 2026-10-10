#!/usr/bin/env bash
# Builds the ci target into the GHCR registry cache for the warm-toolchain-cache
# job (.github/workflows/toolchain-cache.yml).
#
# Docker Hub allows 100 anonymous manifest GETs per hour per IP (HEAD requests
# are not counted, measured 2026-10-10), and GitHub-hosted runners share egress
# IPs, so the ubuntu base pull can return 429 until the window frees. Only that
# failure is retried: any other build error is real and fails at once. A retry
# reuses the layers already in the cache, so it costs little.
#
# The loop lives here, not inline in the workflow, so it runs (and can be
# exercised with a stub docker) on its own.
#
# Usage: CACHE_REF=ghcr.io/<owner>/<repo>-toolchain bash scripts/warm-toolchain-cache.sh
set -uo pipefail

: "${CACHE_REF:?set CACHE_REF to the registry cache ref}"

cd "$(dirname "$0")/.." || exit 1

# Sleeps total about 52 min, enough to outlast one hourly rate-limit window.
delays=(120 300 600 900 1200)
log="$(mktemp)"
trap 'rm -f "$log"' EXIT

attempt=0
while :; do
  status=0
  docker buildx build --target ci \
    --cache-from "type=registry,ref=$CACHE_REF,ignore-error=true" \
    --cache-to "type=registry,ref=$CACHE_REF,mode=max,ignore-error=true" \
    . 2>&1 | tee "$log" || status=$?
  [ "$status" -eq 0 ] && exit 0

  if ! grep -qiE '429 Too Many Requests|toomanyrequests' "$log"; then
    echo "::error::toolchain cache build failed (not a registry rate limit)" >&2
    exit "$status"
  fi
  if [ "$attempt" -ge "${#delays[@]}" ]; then
    echo "::error::Docker Hub rate limit persisted through ${attempt} retries" >&2
    exit "$status"
  fi

  delay=${delays[$attempt]}
  attempt=$((attempt + 1))
  echo "::warning::Docker Hub rate limit, retry ${attempt}/${#delays[@]} in ${delay}s" >&2
  sleep "$delay"
done
