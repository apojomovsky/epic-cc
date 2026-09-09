#!/usr/bin/env bash
# SDCC regression-suite pass rate (docs/35 section 4, Tier 3): runs SDCC's
# own vendored regression suite for the pic14 and pic16 ports under gpsim
# and reports the pass rate as context next to the parity table. The suite
# is GPL and lives in the image only (PIC8_SDCC_REGRESSION); nothing here
# is committed or shipped. Informational, never a gate: SDCC's pic ports
# have acknowledged bugs (docs/35 section 1). Run inside the image from the
# mounted /workspace: bash scripts/sdcc-regression.sh

set -euo pipefail

REGRESSION="${PIC8_SDCC_REGRESSION:-/usr/local/share/sdcc/regression}"
if [ ! -d "$REGRESSION" ]; then
  echo "::error::SDCC regression suite not found; the image must vendor it (see Dockerfile)" >&2
  exit 2
fi
if ! command -v gpsim >/dev/null 2>&1; then
  echo "::error::gpsim not on PATH; the pic14/pic16 ports run under gpsim" >&2
  exit 2
fi

cd "$REGRESSION"

# Run each port and capture its aggregate summary line. The suite's
# spec files already pass SDCC_BIN_PATH via the make invocation; the
# support.c preproc_asm workaround was applied at image build time.
for port in pic14 pic16; do
  echo "=== $port regression suite ==="
  # The port excludes are the suite's own (pic14/pic16 are excluded from
  # the suite's default ALL_PORTS, so we run them explicitly). A non-zero
  # make under the suite's `-` error semantics means individual cases
  # failed, which is expected for SDCC's pic ports (docs/35 section 1);
  # the report below is the point, not the make status.
  make "test-$port" SDCC_BIN_PATH=/usr/local/bin || true
done

echo ""
echo "=== SDCC regression-suite pass rate ==="
sdcc --version | head -1
for port in pic14 pic16; do
  sum="$REGRESSION/results/$port.sum"
  if [ ! -f "$sum" ]; then
    echo "$port: missing summary (suite did not run)" >&2
    exit 1
  fi
  cat "$sum"
done

# Write a pass-rate line next to the parity table, stamped with the SDCC
# version (provenance, docs/35 section 7). GITHUB_STEP_SUMMARY is a
# runner-host path not mounted into the container, so we write to a file
# in the bind-mounted /workspace and let the host step append it.
if [ -n "${SUMMARY_FILE:-}" ]; then
  {
    echo ""
    echo "### SDCC regression-suite pass rate"
    echo "vers: $(sdcc --version | head -1)"
    for port in pic14 pic16; do
      line="$(grep -E '^Summary for' "$REGRESSION/results/$port.sum" | head -1 || true)"
      if [ -n "$line" ]; then
        # 0 failures of N tests -> 100% pass.
        failures="$(echo "$line" | sed -E 's/.* ([0-9]+) failures.*/\1/')"
        tests="$(echo "$line" | sed -E 's/.* ([0-9]+) tests.*/\1/')"
        if [ "${tests:-0}" -gt 0 ] 2>/dev/null; then
          pass="$(awk -v f="$failures" -v t="$tests" 'BEGIN{printf "%.1f", (t-f)*100/t}')"
        else
          pass="-"
        fi
        echo "- $port: $pass% pass ($failures failures / $tests tests)"
      fi
    done
  } >> "$SUMMARY_FILE"
fi
