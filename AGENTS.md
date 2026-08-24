# AGENTS.md

`epic-cc` is a whole-program C compiler for 8-bit Microchip PIC
microcontrollers (PIC14/PIC18), written in Rust. It uses clang 20.1.8
as an out-of-process front end (`-S -emit-llvm`, text in) and owns every
stage from LLVM IR text down to Intel HEX: no external assembler or
linker in the product. Targets the PIC16F877A today; PIC18 support is
landing incrementally.

## The one idea that matters most

**The allocator is the compiler.** PIC14 has no stack: locals are
statically allocated and overlaid across the whole-program call graph,
which forces whole-program compilation by construction. Every stage is
a separate crate with a **diffable text boundary** (`.ll` in, `.asm`/HEX
out), so a miscompile bisects to a stage before anyone reads code.
Read `docs/30-distribution-design.md` (the docker toolchain) and
`docs/01-target-pic14.md` (why the target is hard) before touching
code.

The front end is a **pinned clang 20.1.8**: its version is part of our
input format, so the pin is a migration, never housekeeping. The
release bundles ship it; the dev container builds it from the
digest-pinned source tarball.

## Build & toolchain

First-time setup: `make bootstrap` (checks host deps, installs git
hooks, builds the dev image; `make doctor` reports what is missing
without changing anything). Everything runs inside the docker dev
image. **No rustup, clang, or gpasm on the host; never install them.**
The `ci` Dockerfile stage is an empty alias of `dev` (identical
filesystem), so running locally in the dev image IS running in the ci
image. There is no separate local ci image and no reason to build one.

```bash
make image          # build the dev image (slow first time: compiles clang)
make shell          # interactive dev shell (cargo/clang/gpasm inside)
make test           # full suite, the exact script CI runs (ci-test.sh)
make test CRATE=asm # scope to one crate
make compile        # compile add.c to HEX and print it
make exec CMD='cargo test -p fuzz -- --ignored'   # one-off command
make info           # toolchain versions + PIC8_* env vars
make fmt            # cargo fmt in the container
make lint           # clippy, advisory only (never fails)
make check-warnings # cargo build --workspace --all-targets, fails on any warning
make release-bundle VERSION=0.1.0   # Linux release zip into dist/
make setup-hooks    # install git hooks from .githooks/
make help           # all targets
```

The container runs as your uid (files you write stay host-owned), and
cargo caches live under `~/.cache/epic-cc/` (persisted across runs, the
host `target/` is never touched). `make exec` hands its `CMD` to
`bash -c`; avoid double quotes inside it.

## Picking up work

Work across epic-cc, epic-hal and epic-platformio is coordinated by
[epic-tasks](https://github.com/apojomovsky/epic-tasks). Several agents, from
different providers and on different machines, share one GitHub account, so the
board is the only place that knows what is already taken. **Do not choose a
ticket by reading the issue list.**

Run once per machine (and after any env change):

0. `epic-tasks doctor`: checks `EPIC_AGENT_ID`, `EPIC_TASKS_PROJECT`,
   `gh` auth with `project` scope, and board reachability. Fix what it
   reports before claiming.

For every ticket:

1. `epic-tasks next` to see what you may take, `epic-tasks claim <repo>#<n>` to
   take it. Exit 2 means another agent won the race, so go back to `next`.
   Exit 3 means the board is unreachable: do the work and say so in the pull
   request. Exit 4 means stop and ask.
2. Create a worktree under `.worktrees/` and branch as
   `<type>/<issue>-<slug>`, for example `fix/71-switch-default-branch`
   (see Worktrees below, never work on `master`).
3. Work, then run the takeoff ritual (`make pre-pr-check` → `epic-tasks takeoff`).
4. Open the pull request with `Closes #N`, then
   `epic-tasks review <repo>#<n> --pr <url>`.
5. After the PR merges, remove the worktree:
   `git worktree remove .worktrees/<name>`. Never remove a worktree before
   merge, the branch must stay reachable for review.


Set `EPIC_AGENT_ID` (`<runtime>@<host>`) and `EPIC_TASKS_PROJECT` once per
runtime and machine. `claim` refuses to act without an identity, because an
anonymous claim tells the other agents nothing.

An issue also carries `area:*` labels naming the surfaces it touches. Two
tickets sharing an area cannot be worked at the same time even when neither
blocks the other, which is why selection goes through the tool: what is
blocked, taken, or conflicting is decided there, not in this file.

## Worktrees

**All feature work happens in a worktree under `.worktrees/`**, never on
`master`, and worktrees are removed only after the PR merges:

```bash
git fetch origin master
git worktree add .worktrees/<name> -b <branch> origin/master
# ... work, PR, merge ...
git worktree remove .worktrees/<name>
```

Branch names are conventional: `feat/<description>`, `fix/<description>`,
`chore/<description>`, `docs/<description>`. The worktree keeps your
master checkout clean and lets several tasks run in parallel without
touching each other's trees. Squash merging keeps master plan-free.
The default base is the latest `origin/master`; branching off a different
branch is the exception, reserved for multi-step work other tasks build on
in parallel.

Worktree discipline is enforced by the takeoff ritual (`epic-tasks takeoff`
checks you are in a `.worktrees/` worktree and not on `master`).


## Development cycle

Fast inner loop, in the container: `make shell`, edit in the mounted
workspace, `make test CRATE=<crate>` / `make compile`. A build or test
failing with too little output: `make shell` for the full picture.

Every stage boundary is a text artifact. When debugging a miscompile,
bisect stage by stage (`.ll` text → our IR → alloc map → `.asm`) before
reading code. The verification stack is layered and all local:
our own PIC14 simulator (`crates/sim`), a gpasm byte-for-byte
cross-check (oracle only, GPL, never shipped), e2e acceptance programs
through the whole pipeline, and differential fuzzing (`crates/fuzz`).

## Takeoff ritual (before every PR)

Run `make pre-pr-check` before opening a PR. It is a thin wrapper around
`epic-tasks takeoff`, the shared skeleton used by every epic repository
(canonical checks live in `epic-tasks/epic_tasks/takeoff.py`). It checks:

1. Working tree clean, branch not behind `origin/master` (or `$BASE_REF`).
2. **You are in a `.worktrees/` worktree**, not on `master`.
3. **No plan files in the PR's final diff.** Plans live through
   development; the final commit distills load-bearing decisions into
   an ADR (`docs/adr/ADR-00N-<topic>.md` + an index line in
   `docs/03`) and `git rm`s the plan. Squash merging then keeps master
   plan-free. The plan stays visible in the PR's commit history.
4. Commit hygiene: conventional single-line subjects, no trailers,
   no em-dashes, no whitespace errors.
5. **Compiler warnings.** `cargo build --workspace --all-targets` must
   be clean (`make check-warnings`). Hard gate, unlike `make lint`
   (clippy): rustc's own lints (`unused_mut`, `dead_code`,
   `unused_variables`, `non_snake_case`, `unused_assignments`, ...) are
   high-signal and rarely worth silencing, and a `dead_code` warning is
   what surfaces a function a refactor orphaned but never deleted (see
   `crates/alloc`'s history). `.githooks/pre-commit` runs the same
   check on every commit that touches `*.rs`, so this should already
   be clean by the time the ritual runs.
6. **Comment and doc prose review.** `scripts/prose-diff.sh` extracts
   every added comment block and markdown diff hunk in the PR; it
   flags a couple of objective signals (block over 3 lines, a
   hardcoded count or tree dump) but cannot judge content, so it never
   fails the ritual on its own. The agent reads everything the script
   printed against the Expression Conventions below (why not what,
   <= 3 lines unless truly justified, no decoration or iteration
   narrative; docs stay clear and skip volatile facts) and fixes
   what doesn't hold up. `make pre-pr-check PROSE=1` records that the
   review happened.
7. Hooks installed (`make setup-hooks`).
8. `make pre-pr-check TEST=1` also runs the full suite (or `epic-tasks takeoff --test`).

The ritual exits 1 with the exact fix list while blocking items are
outstanding. Don't skip it; the CI gate only covers the suite, not the
ritual. `epic-tasks takeoff --prose` is the same as `PROSE=1`.

## Commit hygiene

- **Conventional Commits, single line, <= 3 lines.**
  `feat(scope): summary`, `fix(...)`, `chore(...)`, `docs(...)`,
  `build(...)`, `ci(...)`, `test(...)`. Scope is usually the crate
  (`isel`, `banking`, `driver`) or `docker`.
- **Never `Co-Authored-By:` or any other trailer, and no em-dashes
  (—).** The commit-msg hook rejects both. Use a comma, a colon, or a
  period instead. Git history is the record; the commit message is
  yours.
- Commit whenever a piece of work is finished; don't batch unrelated
  changes.
- Update the docs a change touches before calling it done.

## Ground rules

- **Approval gates are real.** Brainstorm → design → approve →
  implement. Present a design and stop until you get a yes, even for
  work that looks small.
- **Never reverse-engineer or disassemble XC8 binaries.** XC8 is a
  black-box differential oracle only: compile the same source with
  `xc8-cc` and diff observable behaviour. Its licence forbids more.
- **GPL boundary.** `gputils`/`gpasm` and `gpsim` are GPL: invoking
  them as external processes in tests is fine; linking them into the
  compiler is not.
- **Never commit the reference PDFs.** They are copyrighted and live
  outside the repo (`docs/06-environment.md`).
- **Don't copy the Microchip datasheets into docs either**: link them.
- **Panics are the error surface today.** Unsupported input aborts with
  a precise message; that is deliberate (correct over silent
  miscompile). Don't "fix" panics by emitting wrong code.
- **No force pushes.** Rewriting a branch that already exists on the
  remote drops it for every other agent and clone; the pre-push hook
  refuses it. If the guard is triggered, rebase onto master and get
  the human's explicit go-ahead before re-running with
  `EPIC_FORCE_PUSH_APPROVED=1 git push --force-with-lease`.

## Expression conventions (comments and docs)

### Comments

1. **Why, not what.** Code says what it does; comments carry the
   non-obvious reason, the datasheet fact, the invariant. A comment
   that restates the line below it is deleted.
2. **A comment must earn its lines.** More comment lines than code is a
   smell. Hand-traces survive only where behavior cannot be read from
   the code, compressed to the essential steps.
3. **No decoration.** No `/* --- name --- */` separators, no
   `@file`/`@brief` boilerplate repeating the filename. A 1-3 line
   file header is fine when it adds context (which stage, what it
   rides on).
4. **No narrative.** No "fixed X by doing Y", no iteration or session
   prose. Verification claims about a change belong in the PR and
   commit, not in the tree, where they go stale. Durable toolchain or
   hardware facts (with a date) are a different class and stay.
5. `TODO`/`FIXME` carry a concrete reason or do not exist.
6. **No em-dashes (—) in prose.** Not in comments, docs, or commit
   messages: use a comma, a colon, or a period and a new sentence.
   The exception is ascii-art diagrams, where alignment may force
   them. The pre-pr-check and commit-msg hook enforce this.
   Replacing an em-dash is a judgment call, not a swap: pick the
   replacement (and split or reorder the sentence when needed) so
   the result reads as prose. A mechanical ` — ` -> ` , ` sweep
   produces comma splices; the pre-pr-check flags the ` ,` residue
   as a warning. Prefer a human or a language model for sweeps.

### Rust doc comments (the Doxygen equivalent)

Rust's documentation comments are `///` (item docs, rendered by
`cargo doc`) and `//!` (crate/module docs). The repo convention:

- `//!` at the top of every crate root: what the crate does, its place
  in the pipeline, its text boundary (the stage contract).
- `///` on public items whose behavior isn't obvious from the
  signature. Document the contract: what it does, what it returns,
  what it panics on. No `@param`/`@return` boilerplate: Rust has real
  types; prose contracts beat re-stated signatures.
- Panic and invariant notes belong in the doc comment (e.g. "panics
  if the page assignment straddles a boundary").
- Tests and internal helpers: `///` only when non-obvious.

### Docs lifecycle

1. `docs/NN-*.md` = the numbered design/decision series; `docs/03` is
   the ADR log (ADR-001..008) and the index for newer ADRs. Living
   documents.
2. **Implementation plans** (`docs/superpowers/plans/`) are ephemeral:
   they live through development and are deleted in the final commit
   before merging (the takeoff ritual enforces this). The PR's commit
   history keeps them for archaeology; master never carries them.
   Design docs are not plans: they stay.
3. **Decisions distill into ADRs when worth it** (pragmatism): a bug
   fix usually does not earn an ADR, a feature or an architectural
   change does. Distillation happens in the same final commit that
   deletes the plan. New ADRs go to `docs/adr/ADR-00N-<topic>.md`
   (Status line, decision, rationale, rejected alternatives) with a
   one-line index entry added to `docs/03-decisions.md`.
4. No bitacores: findings narratives and session logs describing
   completed work are deleted. Live gotchas live in
   `docs/09-build-environment.md` (toolchain) or the ADRs (decisions).
5. **Write for a reader who wasn't there.** Clear, easy to follow,
   sized to the point being made: a doc that overwhelms with detail is
   as broken as one that omits the load-bearing fact.
6. **No coupling to volatile facts.** Test counts, file/dir layout, a
   pasted tree, line counts: describe mechanisms, never numbers or a
   snapshot that goes stale on the next merge.
7. **Diagrams earn their place.** A mermaid diagram is welcome where
   it clarifies structure or flow that prose would belabor; skip it
   for anything a sentence already says clearly.
8. Third-party code keeps its own style; these rules are first-party
   only.

## Non-obvious things that will bite you

- **The `.ll` surface is the input format.** clang's version is pinned
  because we parse its text output. Never "just bump" it, and never
  assume a newer clang emits the same IR shapes.
- **`-target msp430` is a datalayout proxy, not a target.** We are not
  generating MSP430 code; it gives us the ABI-independent type
  decisions (8-bit char, 16-bit int/pointers). `-Oz` emits arbitrary
  widths (`i17`): we run `-O1` deliberately.
- **Recursion is a compile error**, and call depth is checked against
  the 8-level hardware stack. Don't add stack frames; that's not a
  limitation to "fix", it's the architecture.
- **Locals are keyed `{func}::{name}`** in the address map. The
  driver's `HashMap` contract both backends look up. Renaming breaks
  the map, not just one backend.
- **BANKSEL between a skip-sensitive test and its branch changes the
  skip target.** Issue #6 territory; the banking pass and the runtime
  routines both know this. Don't "helpfully" reorder.
- **The driver binary is `epic-cc`, not `driver`.** `cargo run -p
  driver` works (the package is still named driver), but anything that
  spawns the binary by name must use `epic-cc` (or
  `CARGO_BIN_EXE_epic-cc` in tests). The fuzz harness's `driver_binary()`
  learned this the hard way.
- **Docker builds only see git-tracked files** when COPYing the
  workspace: stage new files before building a release bundle.

## CI

`.github/workflows/ci.yml` runs the full workspace suite inside the
`ci` image (an alias of `dev`) on every push/PR. The clang layer is
cached in GHCR via the buildx registry cache; the first run on a fresh
cache is ~1h, everything after is minutes. `.github/workflows/
release.yml` builds the release bundles on tags. Both workflows use
`packages: write` + `ignore-error=true` on the registry cache. The
cache is an optimization, never a gate.
