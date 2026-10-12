#!/usr/bin/env bash
# Regenerate the committed size and cycle baseline TOMLs from the ladders.
# Runs inside the dev/ci container (see .github/workflows/baseline-regen.yml),
# never via scripts/ci-test.sh: that script sets BASELINE_ARTIFACT_DIR, which
# makes the ladders measure-only, while regeneration must record.
#
# Usage: bash scripts/baseline-regen.sh (inside the container, repo root)
set -euo pipefail

if [ -n "${BASELINE_ARTIFACT_DIR:-}" ]; then
  echo "::error::BASELINE_ARTIFACT_DIR is set; unset it so the ladders record" >&2
  exit 2
fi

UPDATE_SIZE_BASELINE=1 cargo test -p driver --test size_regression_e2e
UPDATE_CYCLE_BASELINE=1 cargo test -p driver --test cycle_ladder_e2e
