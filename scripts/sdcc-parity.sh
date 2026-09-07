#!/usr/bin/env bash
# SDCC parity differential runner (docs/35, P0). Runs the committed corpus
# (Tier 1 + Tier 2) through both epic-cc and SDCC, compares named outputs,
# and writes a per-program table (flash, RAM, cycles, pass/fail) to the
# GitHub step summary. SDCC is GPL and lives in the image as an external
# oracle only; this script never links or commits SDCC code.
#
# Usage: docker run --rm -v "$PWD:/workspace" -w /workspace epic-cc-ci:latest \
#   bash scripts/sdcc-parity.sh

set -euo pipefail

if ! command -v sdcc >/dev/null 2>&1; then
  echo "::error::sdcc not on PATH; the image must build SDCC 4.6.0 (see Dockerfile)" >&2
  exit 2
fi

# Build the driver and the parity harness inside the container.
cargo build -p driver -p sdcc-parity

# Run the differential test with output captured for the summary.
out="$(cargo test -p sdcc-parity --test differential -- --nocapture 2>&1)" || {
  echo "::error::sdcc-parity differential failed" >&2
  echo "$out" >&2
  exit 1
}
echo "$out"

# Write the per-program table to the step summary when present.
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "## SDCC parity differential"
    echo ""
    echo "| Program | Device | epic flash | sdcc flash | epic RAM | sdcc RAM | epic cyc | sdcc cyc | Result |"
    echo "|---|---|---|---|---|---|---|---|---|"
    echo "$out" | grep -E '^(PASS|FAIL)' | sed -E \
      -e 's/^PASS ([^ ]+) on ([^:]+): epic ([0-9]+)w\/([0-9]+)B\/([0-9]+)cyc sdcc ([0-9]+)w\/([0-9]+)B\/([0-9]+)cyc$/| \1 | \2 | \3 | \6 | \4 | \7 | \5 | \8 | PASS |/' \
      -e 's/^FAIL ([^ ]+) on ([^:]+): (.*)$/| \1 | \2 | - | - | - | - | - | - | FAIL: \3 |/'
  } >> "$GITHUB_STEP_SUMMARY"
fi
