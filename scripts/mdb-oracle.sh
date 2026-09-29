#!/usr/bin/env bash
# MDB execution oracle for superopt sequences (epic-cc#712): emit a replay
# batch, run it under MPLAB SIM via epic-hal's mdb-hex gate, diff against
# the in-tree sim. Microchip tooling stays an external process.
# Usage: mdb-oracle.sh --spec <name> [--tier pr|nightly] [--hal <dir>]
#   [--mutate] [--keep]. --mutate corrupts chunk 0 (check must fail);
#   EPIC_MDB_GATE=1 turns unavailable-mdb from local skip to CI failure.

set -euo pipefail

SPEC=""
TIER="pr"
HAL="${EPIC_HAL_ROOT:-}"
MUTATE=0
KEEP=0
while [ $# -gt 0 ]; do
  case "$1" in
    --spec) SPEC="$2"; shift 2 ;;
    --tier) TIER="$2"; shift 2 ;;
    --hal) HAL="$2"; shift 2 ;;
    --mutate) MUTATE=1; shift ;;
    --keep) KEEP=1; shift ;;
    *) echo "usage: mdb-oracle.sh --spec <name> [--tier pr|nightly] [--hal <dir>] [--mutate] [--keep]" >&2; exit 2 ;;
  esac
done
[ -n "$SPEC" ] || { echo "mdb-oracle: --spec required" >&2; exit 2; }
if [ -z "$HAL" ]; then
  HAL="$(dirname "$(cd "$(dirname "$0")/.." && pwd)")/epic-hal"
fi

fail_or_skip() {
  if [ "${EPIC_MDB_GATE:-0}" = "1" ]; then
    echo "mdb-oracle: FAIL ($1)" >&2
    exit 1
  fi
  echo "mdb-oracle: SKIP ($1)"
  exit 0
}
[ -d "$HAL" ] || fail_or_skip "no epic-hal checkout at $HAL"
command -v docker >/dev/null || fail_or_skip "no docker"
# CI passes both images in and skips the local `make` wrappers (which
# would rebuild the dev image and the license-gated toolchain image).
# Locally both are unset and the Makefile targets run as before.
if [ -z "${MDB_ORACLE_HAL_IMAGE:-}" ]; then
  docker image inspect epic-hal-toolchain:local >/dev/null 2>&1 \
    || fail_or_skip "no epic-hal-toolchain:local image"
fi

# Run the driver bin in the epic-cc image.
cc_bin() {
  if [ -n "${MDB_ORACLE_CC_IMAGE:-}" ]; then
    docker run --rm -v "$PWD:/workspace" -w /workspace \
      ${MDB_ORACLE_CC_CACHE_MOUNT:-} "$MDB_ORACLE_CC_IMAGE" \
      cargo run -q -p superopt --bin mdb_oracle -- "$@"
  else
    make exec CMD="cargo run -q -p superopt --bin mdb_oracle -- $*"
  fi
}

# Run one HEX under MPLAB SIM in the epic-hal toolchain image.
hal_mdb() {
  if [ -n "${MDB_ORACLE_HAL_IMAGE:-}" ]; then
    docker run --rm -v "$HAL:/repo" -w /repo "$MDB_ORACLE_HAL_IMAGE" \
      scripts/mdb-hex-run.sh "$1" "$2" "$3" "$4" > "$5" 2>&1
  else
    make -C "$HAL" mdb-hex HEX="$1" DEVICE="$2" WAIT_MS="$3" EXTRA_MDB="$4" > "$5" 2>&1
  fi
}

OUT_BASE="scratch/mdb-oracle/$SPEC-$TIER"
rm -rf "$OUT_BASE"
MUT_FLAG=""
[ "$MUTATE" -eq 1 ] && MUT_FLAG="--mutate"
COUNT="$(cc_bin count --spec $SPEC --tier $TIER | grep -E '^[0-9]+ [0-9]+$')"
read -r CASES PER_CHUNK <<< "$COUNT"
# Zero cases must fail, never vacuous-pass: an empty tier list would
# otherwise skip the chunk loop and exit 0 with no hardware evidence.
[ "$CASES" -gt 0 ] || { echo "mdb-oracle: FAIL: $SPEC/$TIER has no cases" >&2; exit 1; }
NCHUNKS=$(( (CASES + PER_CHUNK - 1) / PER_CHUNK ))
[ "$MUTATE" -eq 1 ] && NCHUNKS=1
echo "mdb-oracle: $SPEC/$TIER: $CASES cases in $NCHUNKS chunk(s)"
trap '[ "$KEEP" -eq 1 ] || rm -f "$HAL/build/epiccc/mdb-oracle.hex"' EXIT
i=0
while [ "$i" -lt "$NCHUNKS" ]; do
  OUT="$OUT_BASE/chunk$i"
  mkdir -p "$OUT"
  cc_bin emit --spec $SPEC --tier $TIER --chunk $i/$NCHUNKS --out-dir /workspace/$OUT $MUT_FLAG
  mkdir -p "$HAL/build/epiccc"
  cp "$OUT/prog.hex" "$HAL/build/epiccc/mdb-oracle.hex"
  EXTRA_MDB="$(awk '{printf "x /1xbr %s\\n", $1}' "$OUT/reads.txt")"
  # Deterministic backstop: the runner's wall-clock run+wait once stalled
  # mid-preamble under load. Overshoot is safe (spin terminator). Literal
  # \n, not a real newline: make splits real newlines out of the quoted
  # recipe argument and the shell chokes (this bit us).
  EXTRA_MDB="stepi $(cat "$OUT/stepi.txt")\\n$EXTRA_MDB"
  if [ -n "${MDB_ORACLE_HAL_IMAGE:-}" ]; then
    HEX_ARG="/repo/build/epiccc/mdb-oracle.hex"
  else
    HEX_ARG="build/epiccc/mdb-oracle.hex"
  fi
  if ! hal_mdb "$HEX_ARG" PIC18F4550 5000 "$EXTRA_MDB" "$OUT/mdb.log"; then
    echo "mdb-oracle: chunk $i: mdb session failed, see $OUT/mdb.log" >&2
    tail -20 "$OUT/mdb.log" >&2
    exit 1
  fi
  if cc_bin check --out-dir /workspace/$OUT --mdb-log /workspace/$OUT/mdb.log; then
    echo "mdb-oracle: chunk $i/$NCHUNKS PASS"
  elif [ "$MUTATE" -eq 1 ]; then
    echo "mdb-oracle: mutation caught as required"
    exit 0
  else
    exit 1
  fi
  i=$((i + 1))
done
if [ "$MUTATE" -eq 1 ]; then
  echo "mdb-oracle: FAIL: mutated program passed check" >&2
  exit 1
fi
