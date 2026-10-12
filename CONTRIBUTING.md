# Contributing to epic-cc

Welcome. `epic-cc` is a from-scratch C compiler for 8-bit PIC microcontrollers,
fully open source and MIT licensed. You do not need to know PIC assembly to
help: there is documentation work, test programs, device files, and tooling
around a fairly small Rust codebase. This page takes you from clone to first
pull request. Anything here that contradicts `AGENTS.md`, loses: that file is
the normative rulebook (it is written for autonomous agents, so it reads that
way), and this page links to it rather than repeating it.

## Where help is welcome

Browse the [open issues](https://github.com/apojomovsky/epic-cc/issues).
Good starting points carry the [`good first issue`](https://github.com/apojomovsky/epic-cc/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22)
or [`help wanted`](https://github.com/apojomovsky/epic-cc/issues?q=is%3Aissue+is%3Aopen+label%3A%22help+wanted%22)
labels. If an issue already looks taken (an assignee, or recent comments saying
so), pick another one or ask on it before starting, since several contributors
may be working at once. If you find a bug with no issue, file one: a minimal C
file plus the exact command you ran is the ideal report.

## Setup: everything runs in Docker

There is deliberately nothing to install on your machine. The compiler depends
on a pinned clang, a specific Rust toolchain, and a PIC assembler used only as
a test oracle, so all of it lives in a Docker dev image. You need Docker, a
checkout, and three commands:

```bash
make bootstrap   # once: checks host deps, installs git hooks, builds the image
make shell       # development shell inside the image: cargo, clang, gpasm
make test        # the full suite, exactly what CI runs
```

The first build is slow (it compiles clang from source), later runs reuse a
cache. Your files stay owned by you, and nothing lands system-wide, so there
is never a reason to install rustup, clang, or gpasm on the host. `make help`
lists the rest, and `docs/09-build-environment.md` documents the pinned
versions. To run a single command without opening a shell:

```bash
make exec CMD='cargo test -p asm'
```

## Running the tests

`make test` runs `scripts/ci-test.sh`, the same script CI runs, so a local
pass means a CI pass. While iterating, scope it down and compile small
programs directly:

```bash
make test CRATE=asm                                                # one crate only
make compile FILE=crates/driver/tests/fixtures/add.c TARGET=p16f877a  # C to HEX, printed
```

Two suites deserve a special mention. The **size ladder** and the **cycle
ladder** (`crates/driver/tests/size_regression_e2e.rs` and
`cycle_ladder_e2e.rs`) record how much flash, RAM, and time each benchmark
program uses, in `crates/driver/tests/fixtures/`. A pull request does not
edit those baselines. CI measures the ladders and `scripts/baseline_delta.py`
judges the move against master's baseline: a net win (no total grows, at
least one shrinks) passes; flash growth up to 8 words or cycle growth up to
1% on a single row passes when the PR body names it on a line of its own,
`delta-ok: <row>`; RAM growth, a larger regression, a net loss, or a new or
removed row holds for a human. After a merge, the `baseline-regen` workflow
re-measures the ladders on master and commits the refreshed TOMLs as
`chore(baseline): regenerate`, so the next pull request is judged against
current numbers. To preview what the bot will commit, run
`bash scripts/baseline-regen.sh` in the dev container and read the diff;
never commit the result from a pull request branch.

## Conventions, in brief

The full rules live in `AGENTS.md`. The ones every human contributor meets:

- **Commits** follow Conventional Commits with a short single-line subject,
  for example `fix(isel): handle empty struct return`. The scope is usually
  the crate you touched.
- **Prose has no em-dashes**, not in comments, docs, or commit messages. Use
  a comma, a colon, or a new sentence.
- **Work on a branch in a worktree**, never on `master`. Short version:
  `git worktree add .worktrees/<name> -b <type>/<slug> origin/master`,
  where `<type>` is `fix`, `feat`, `docs`, and so on.
- **Run `make pre-pr-check` before opening a pull request.** It checks branch
  hygiene, compiler warnings (which must be zero), comment prose, and hooks.
- **Every change gets a separate reviewer** before it merges. Expect review
  findings to be addressed before takeoff, not after.

## Where the docs live

- `docs/08-status-and-next-steps.md`: the current-state map. Start here.
- `docs/00-charter.md`: what the project is and is not.
- `docs/03-decisions.md` plus `docs/adr/`: decisions with rejected
  alternatives. Architectural changes add an ADR.
- `docs/04-pipeline-design.md`, `docs/12-backend-design.md`: how the
  compiler fits together.
- `docs/05-verification.md`: the simulator, oracle, and fuzzing story.
- `docs/32-adding-a-device.md`: the runbook for supporting a new part.

## Asking questions

Open an issue with the
[`question`](https://github.com/apojomovsky/epic-cc/issues?q=is%3Aissue+is%3Aopen+label%3Aquestion)
label. There are no bad first questions here: if the answer is not in the
docs, the docs probably need the answer too.
