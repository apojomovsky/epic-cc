#!/usr/bin/env bash
# Takeoff ritual: run before opening a PR. Verifies the branch meets the
# repo's merge conventions and prints exactly what still has to be done.
# Exits 1 while blocking items are outstanding; advisory items warn only.
#
# The load-bearing rule: implementation plans (docs/superpowers/plans/)
# live in the PR's commit history for archaeology but must not reach
# master's tree. Distill load-bearing decisions into
# docs/adr/ADR-00N-<topic>.md (Status line, rationale, rejected
# alternatives; add a one-line index entry to docs/03-decisions.md),
# then `git rm` the plan in a final commit. Squash merging then keeps
# master plan-free automatically.
#
# Usage: bash scripts/pre-pr-check.sh [--test]
#   --test   also run the full suite (make test) ; slow, opt-in
#   BASE_REF overrides the base branch (forks: BASE_REF=<fork>/master)

set -uo pipefail

BASE_REF="${BASE_REF:-origin/master}"
RUN_TESTS=0
[ "${1:-}" = "--test" ] && RUN_TESTS=1

FAILS=0
WARNS=0

say()  { printf '%s\n' "$*"; }
ok()   { say "  [ok] $*"; }
warn() { say "  [warn] $*"; WARNS=$((WARNS + 1)); }
fail() { say "  [FAIL] $*"; FAILS=$((FAILS + 1)); }

branch=$(git branch --show-current)
say "== Takeoff ritual: ${branch:-detached HEAD} =="
say ""

# ── 1. Working tree must be clean ──────────────────────────────────
if [ -n "$(git status --porcelain)" ]; then
  fail "working tree has uncommitted changes (commit or stash first)"
else
  ok "working tree clean"
fi

# ── 2. Branch must not be behind the base ──────────────────────────
if git rev-parse --verify -q "$BASE_REF" >/dev/null 2>&1; then
  behind=$(git rev-list --count "HEAD..$BASE_REF")
  if [ "$behind" -gt 0 ]; then
    warn "branch is $behind commit(s) behind $BASE_REF ; rebase before merging"
  else
    ok "up to date with $BASE_REF"
  fi
else
  warn "$BASE_REF not found ; fetch first: git fetch origin master"
fi

# ── 3. Implementation plans must not reach master ──────────────────
plans=$(git diff --name-only "$BASE_REF"...HEAD -- docs/superpowers/plans/ 2>/dev/null)
if [ -n "$plans" ]; then
  fail "plan file(s) in the PR's final diff (must not reach master):"
  for p in $plans; do
    say "    - $p"
  done
  say "    Fix:"
  say "      1. Distill load-bearing decisions/rulings into"
  say "         docs/adr/ADR-00N-<topic>.md (Status line, rationale,"
  say "         rejected alternatives) and add a one-line index entry"
  say "         to docs/03-decisions.md."
  say "      2. git rm the plan in a final commit."
  say "    The plan stays in the PR's commit history (archaeology);"
  say "    master must not carry it."
else
  ok "no plan files in the PR diff"
fi

# ── 4. Commit hygiene: conventional, single-subject, no trailers ───
n=$(git rev-list --count "$BASE_REF..HEAD" 2>/dev/null || echo 0)
if [ "$n" -eq 0 ]; then
  fail "no commits ahead of $BASE_REF ; nothing to PR"
else
  say "checking $n commit(s):"
  while IFS= read -r -d '' hash; do
    IFS= read -r -d '' subject || break
    IFS= read -r -d '' body || break
    short=${hash:0:7}
    if [[ "$subject" == Merge\ * ]]; then
      ok "$short merge commit (git-generated, skipped)"
      continue
    fi
    if [[ "$subject" =~ ^(feat|fix|chore|docs|build|ci|test|refactor|style|perf|revert|plan)(\([a-z0-9-]+\))?!?: ]]; then
      ok "$short conventional"
    else
      fail "$short not conventional: $subject"
    fi
    [ ${#subject} -gt 72 ] && warn "$short subject ${#subject} chars (keep <= 72)"
    nl=$(printf '%s' "$body" | grep -c . || true)
    [ "$nl" -gt 3 ] && warn "$short body $nl lines (keep <= 3)"
    if printf '%s\n%s' "$subject" "$body" | grep -qiE '^(co-authored-by|coauthored-by|authored-by):'; then
      fail "$short carries a Co-Authored-By/trailer (forbidden, AGENTS.md)"
    fi
    if printf '%s\n%s' "$subject" "$body" | grep -q '—'; then
      fail "$short contains an em-dash in the commit message (forbidden; use a comma, colon, or period)"
    fi
  done < <(git log "$BASE_REF..HEAD" --format='%H%x00%s%x00%b%x00')
fi

# ── 5. Whitespace errors in the PR diff ────────────────────────────
if out=$(git diff "$BASE_REF"...HEAD --check 2>&1); then
  ok "no whitespace errors"
else
  printf '%s\n' "$out" | head -10
  fail "whitespace errors in the diff (above); fix and re-stage"
fi

# ── 6. Em-dashes in the PR's prose diff (not ascii art) ────────────
# Only ADDED lines are scanned: context lines may carry pre-existing
# em-dashes from lines the PR merely touches. AGENTS.md, this script,
# and docs/03 are excluded by design: AGENTS.md documents the rule
# (and needs the character), the script's grep patterns carry it, and
# docs/03's ADR titles use the em-dash as a structural separator
# (ADR-001..008 convention).
emdashes=$(git diff "$BASE_REF"...HEAD -- . \
  ':(exclude)docs/superpowers/plans/' \
  ':(exclude)AGENTS.md' ':(exclude)CLAUDE.md' \
  ':(exclude)scripts/pre-pr-check.sh' \
  ':(exclude)docs/03-decisions.md' 2>/dev/null \
  | grep -nE '^\+[^+].*—' | head -10)
if [ -n "$emdashes" ]; then
  printf '    %s\n' "$emdashes"
  fail "em-dashes (—) in the PR's diff (forbidden in prose; use a comma,"
  fail "      colon, or period. Exception: ascii-art/diagrams)."
else
  ok "no em-dashes in the diff"
fi

# ── 7. Hooks installed (make setup-hooks) ──────────────────────────
hooks=$(git rev-parse --git-path hooks)
if [ ! -x "$hooks/pre-commit" ]; then
  warn "git hooks not installed ; run: make setup-hooks"
else
  ok "git hooks installed"
fi

# ── 8. Suite (opt-in via --test) ───────────────────────────────────
if [ "$RUN_TESTS" -eq 1 ]; then
  say ""
  say "== running the full suite (make test) =="
  if make test; then
    ok "full suite passed"
  else
    fail "full suite failed"
  fi
else
  warn "suite not run ; re-run with --test (make pre-pr-check TEST=1) before merging"
fi

say ""
if [ "$FAILS" -gt 0 ]; then
  say "== $FAILS blocking item(s), $WARNS advisory ; fix and re-run =="
  exit 1
fi
say "== ritual clean ($WARNS advisory) ; ready to open the PR =="
exit 0
