#!/usr/bin/env bash
# Fetch the PR body at run time, verbatim, into a file.
#
# The baseline judge names its `delta-ok:` exceptions from the PR body,
# and the event payload's copy is the one from the run's first attempt:
# after a body edit, a re-run would still judge the old text. Reading
# the body now makes the re-run see the edit (epic-cc#1440).
#
# usage: fetch_pr_body.sh <pr-number> <out-file> [owner/repo]
set -euo pipefail

pr="${1:?usage: fetch_pr_body.sh <pr-number> <out-file> [owner/repo]}"
out="${2:?usage: fetch_pr_body.sh <pr-number> <out-file> [owner/repo]}"
repo="${3:-${GITHUB_REPOSITORY:?fetch_pr_body.sh needs GITHUB_REPOSITORY or a repo argument}}"

# `--jq .body` prints the raw body; without it the JSON envelope would
# land in the file and no `delta-ok:` line could ever match. A null body
# prints as empty, never the word "null". A failed read fails the step:
# judging a body that could not be read would blame the wrong text.
if ! gh pr view "$pr" --repo "$repo" --json body --jq '.body // ""' > "$out"; then
  echo "fetch_pr_body: could not read the body of PR $pr" >&2
  exit 1
fi
