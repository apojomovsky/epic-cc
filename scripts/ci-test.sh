#!/usr/bin/env bash
# Workspace test gate for CI (and for local use): runs every workspace
# crate's tests in its own `cargo test -p <crate>` invocation, so a failing
# crate is attributed by name instead of burying the failure inside one
# workspace-wide run, and writes a PASS/FAIL table to the GitHub step
# summary when $GITHUB_STEP_SUMMARY is set (a no-op locally, so this same
# script is the local gate too).
#
# The loop lives here, not inline in .github/workflows/ci.yml, for the same
# reason epic-hal keeps its CI loops in scripts/ci-target-*.sh: a real
# script file has no YAML-quoting problems and can be run (and shellchecked)
# on its own.
#
# Does not stop at the first failure (fail-fast:false equivalent); exits 1
# if anything failed.
#
# Usage: docker run --rm -v "$PWD:/workspace" -w /workspace epic-cc-ci:latest bash scripts/ci-test.sh

set -uo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  echo "::error::cargo not on PATH; run inside the dev container (see docs/09-build-environment.md)" >&2
  exit 2
fi

# The crate list comes from cargo metadata so a new workspace member is
# picked up automatically. python3, not jq: jq is not in the dev image
# (the Dockerfile deliberately does not include it); python3 is.
crates="$(cargo metadata --no-deps --format-version 1 2>/dev/null \
  | python3 -c 'import json, sys; print("\n".join(sorted(p["name"] for p in json.load(sys.stdin)["packages"])))')" || {
  echo "::error::cargo metadata failed" >&2
  exit 2
}

if [ -z "$crates" ]; then
  echo "::error::no workspace crates found" >&2
  exit 2
fi

fail=0

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "| Crate | Result |"
    echo "|---|---|"
  } >> "$GITHUB_STEP_SUMMARY"
fi

for crate in $crates; do
  echo "::group::${crate}"
  if cargo test -p "$crate" --no-fail-fast; then
    echo "PASS: ${crate}"
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
      echo "| ${crate} | PASS |" >> "$GITHUB_STEP_SUMMARY"
    fi
  else
    echo "FAIL: ${crate}"
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
      echo "| ${crate} | FAIL |" >> "$GITHUB_STEP_SUMMARY"
    fi
    fail=1
  fi
  echo "::endgroup::"
done

exit "$fail"
