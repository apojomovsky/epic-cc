#!/usr/bin/env bash
# Runs `docker buildx build` for every job that builds the ci or release image,
# against the GHCR registry cache. Docker Hub's anonymous limit (100 GETs per IP
# per hour, HEADs uncounted) can 429 the ubuntu base pull on shared runner IPs,
# and registry-1.docker.io returns transient 502/503/504 under load. Only those
# failures are retried, sleeping about 52 min in total to outlast one 429 window.
# Other errors fail at once.
# Usage: CACHE_REF=ghcr.io/<owner>/<repo>-toolchain bash scripts/warm-toolchain-cache.sh --target ci [flags]
# Flags go to `docker buildx build`; the cache flags and the . context are added here.
set -uo pipefail

: "${CACHE_REF:?set CACHE_REF to the registry cache ref}"
[ "$#" -gt 0 ] || { echo "usage: warm-toolchain-cache.sh <docker buildx build flags>" >&2; exit 2; }

cd "$(dirname "$0")/.." || exit 1

# Sleeps total about 52 min, enough to outlast one hourly rate-limit window.
delays=(120 300 600 900 1200)
log="$(mktemp)"
trap 'rm -f "$log"' EXIT

attempt=0
while :; do
  status=0
  docker buildx build \
    --cache-from "type=registry,ref=$CACHE_REF,ignore-error=true" \
    --cache-to "type=registry,ref=$CACHE_REF,mode=max,ignore-error=true" \
    "$@" . 2>&1 | tee "$log" || status=$?
  [ "$status" -eq 0 ] && exit 0

  # A 429 only counts on the final log line, the ERROR BuildKit prints
  # last, so an earlier tolerated 429 warning cannot retry an unrelated
  # failure for an hour. A 5xx with a reason phrase only counts on a line
  # naming a registry, and a bare 502/503/504 only at the end of the
  # final line naming a registry after a status keyword, keeping byte
  # counts and digests out of the match.
  last="$(grep . "$log" | tail -n 1)"
  if ! printf '%s\n' "$last" | grep -qiE '429 Too Many Requests|toomanyrequests' \
    && ! grep -iE 'docker\.io|registry-1|ghcr\.io' "$log" | grep -iE '502 Bad Gateway|503 Service Unavailable|504 Gateway Timeout' >/dev/null \
    && ! printf '%s\n' "$last" | grep -iE 'docker\.io|registry-1|ghcr\.io' | grep -qiE 'status.*[^0-9]50[234][[:space:]]*$'; then
    echo "::error::toolchain cache build failed (not a transient registry error)" >&2
    exit "$status"
  fi
  if [ "$attempt" -ge "${#delays[@]}" ]; then
    echo "::error::registry transient error persisted through ${attempt} retries" >&2
    exit "$status"
  fi

  delay=${delays[$attempt]}
  attempt=$((attempt + 1))
  echo "::warning::transient registry error (429 or 5xx), retry ${attempt}/${#delays[@]} in ${delay}s" >&2
  sleep "$delay"
done
