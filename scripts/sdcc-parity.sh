#!/usr/bin/env bash
# SDCC parity differential runner (docs/35, P0). Runs the committed corpus
# (Tier 1 + Tier 2) through both epic-cc and SDCC, compares named outputs,
# and writes a per-program table (flash, RAM, cycles, pass/fail) plus the
# aggregate ratios to the GitHub step summary. The same run gates: the
# regression_gate test fails on any ratio regression or arbitration change
# against the committed baseline.toml. SDCC is GPL and lives in the image
# as an external oracle only; this script never links or commits SDCC code.
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

# One invocation runs both tests: the differential reports the corpus
# table (informational), the regression gate fails the run on a baseline
# regression or an arbitration change.
out="$(cargo test -p sdcc-parity -- --nocapture --test-threads=1 2>&1)" || {
  echo "::error::sdcc-parity failed (differential or ratio gate)" >&2
  echo "$out" >&2
  exit 1
}
echo "$out"

# Write the tool versions, per-program table and aggregate ratios to the
# step summary (the versions line stamps the table, docs/35 section 7).
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "$out" | grep -o 'versions: .*' | head -1 | sed 's/^/`/;s/$/`/' || true
    echo ""
    echo "| Program | Device | epic flash | sdcc flash | epic RAM | sdcc RAM | epic cyc | sdcc cyc | Result |"
    echo "|---|---|---|---|---|---|---|---|---|"
    echo "$out" | grep -E '^(PASS|EXCLUDED)' | sed -E \
      -e 's/^PASS ([^ ]+) on ([^:]+): epic ([0-9]+)w\/([0-9]+)B\/([0-9]+)cyc sdcc ([0-9]+)w\/([0-9]+)B\/([0-9]+)cyc$/| \1 | \2 | \3 | \6 | \4 | \7 | \5 | \8 | PASS |/' \
      -e 's/^EXCLUDED ([^ ]+) on ([^ ]+) \(([^)]*)\).*/| \1 | \2 | - | - | - | - | - | - | \3 |/' || true
    echo "$out" | grep -E '^aggregate' | sed 's/^/`/;s/$/`/' || true
  } >> "$GITHUB_STEP_SUMMARY"
fi
