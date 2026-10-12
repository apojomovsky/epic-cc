#!/usr/bin/env bash
# Regenerate the committed baseline TOMLs from the ladders, inside the
# dev/ci container (ci-test.sh sets BASELINE_ARTIFACT_DIR for measure-only;
# regeneration must record, so it never runs through that script).
# Manual refresh only (epic-cc#1135): the delta gate measures the base
# commit, these TOMLs are a local record. Refresh from a clean master
# worktree and open a PR, never from a PR branch.
# Usage: bash scripts/baseline-regen.sh (inside the container, repo root)
set -euo pipefail

if [ -n "${BASELINE_ARTIFACT_DIR:-}" ]; then
  echo "::error::BASELINE_ARTIFACT_DIR is set; unset it so the ladders record" >&2
  exit 2
fi

UPDATE_SIZE_BASELINE=1 cargo test -p driver --test size_regression_e2e
UPDATE_CYCLE_BASELINE=1 cargo test -p driver --test cycle_ladder_e2e
