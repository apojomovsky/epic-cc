# ADR-027 -- Bank-aware instruction scheduler: a new `schedule` pass

**Status:** Accepted 2026-09-04<br>
**Decides:** `epic-cc#210`<br>
**Parent:** `docs/02-prior-art.md` (§5, the CASES'06 BANKSEL-minimization reference this ADR clarifies); `docs/12-backend-design.md`, `docs/04-pipeline-design.md`, `docs/01-target-pic14.md` (the same reference repeated)

## Decision

A new crate, `crates/schedule`, sits between `isel` and `banking` in the
PIC14 pipeline (`isel -> schedule -> banking -> peephole -> page-fit ->
asm`). It reorders small, provably-independent instruction groups within a
straight-line region to reduce the bank switches `banking::assign_banks`
must later insert, without moving any instruction across a label, `CALL`,
verbatim `; --- asm start/end ---` block, or a skip-conditional
(`BTFSC`/`BTFSS`/`INCFSZ`/`DECFSZ`) and its immediate successor.

`pub fn schedule(device: &Device, asm: &str) -> String`, matching every
other PIC14 stage's shape (`banking::assign_banks`, `peephole::optimize`):
`&str` in, `String` out, plus a `src/bin/schedule.rs` binary and a
`[[bin]]` entry in `Cargo.toml`, the same convention `crates/banking` and
`crates/peephole` already use. `schedule` depends on `banking` as a library
to reuse its bank-classification helpers (`operand_bank`, `bank_op_effect`,
`SKIP_OPS`, `LITERAL_OPS`, made `pub`) rather than duplicating PIC14's
bank arithmetic a second time; running before `banking` in the pipeline is
a `main.rs` ordering fact, independent of the Cargo dependency direction.

Phase 1 (this ADR's only committed scope) implements exactly one
transform: hoist or sink a single differently-banked instruction that sits
between two same-bank neighbors ("a singleton excursion"), when and only
when it is not a skip op or skip target, touches no flag the surrounding
code reads, has no W-register dependency on its neighbors, and has no
file-register read/write hazard against anything in its move path. General
list scheduling, multi-instruction excursions, flag-chain reasoning, and
cross-`CALL` scheduling are explicitly out of scope until phase 1's
measured results justify the investment.

## Rationale

* **The referenced "published 2-approximation" algorithm does not apply to
  this problem, and conflating the two would produce the wrong design.**
  `docs/02-prior-art.md` §5's CASES'06 reference is an optimal-*placement*
  dataflow analysis: given a *fixed* instruction order, decide where
  `BANKSEL`s are truly needed (a PRE-style, available-expressions
  generalization of what `#213`'s `func_exit_bank`/`label_provable_banks`
  already do for `CALL`s and labels). It never reorders code. epic-cc#210's
  own text flagged this as unconfirmed ("confirm what algorithm is
  actually being referenced"); grepping `crates/` and `docs/` found no
  existing design, code, or paper reference for reordering instructions to
  group bank accesses. This is new ground for the compiler, not an
  implementation of already-planned work, and the ADR exists specifically
  to make that gap and its consequences explicit before code is written.
* **A new crate, not a module inside `isel` or `banking`.** This repo's
  central architectural rule is that every pipeline stage is a separate
  crate with a diffable text boundary (`.ll` in, `.asm`/HEX out), so a
  miscompile bisects to a stage before anyone reads code. `isel` currently
  emits straight to text (`Gen::out: Vec<String>`, no retained per-line
  operand/def-use metadata); folding scheduling into `isel` would collapse
  emission and reordering into one undiffable step, losing exactly the
  property that let `#213`/`#217`/`#214` each be verified and bisected in
  isolation.
* **Between `isel` and `banking`, not after.** `banking` is what turns bank
  *demand* into `BANKSEL` *text*; scheduling after it means re-deriving
  which `BANKSEL`s are now redundant, reinventing `banking`'s own
  straight-line dedup. `peephole`'s PCLATH-pair elision is similarly
  order-sensitive, so scheduling must precede it too.
* **Phase 1 is deliberately narrower than general scheduling.** Fresh
  measurement (isel's own pre-banking output, `hal-pic16-encoder-full`)
  found 471 mid-block bank switches, 271 (58%) of them "singleton
  excursions." A hand-traced real example (`EPIC_IRQ_Enable`) confirmed a
  legal, safe reorder exists and removes a switch. Starting from the exact
  hand-verified shape, with a small, enumerable, fully oracle-verified test
  suite, produces a real measured data point (real headroom, real risk,
  real payoff after the `.align`/`.table` absorption trap) before any
  larger investment, the same discipline `#213`/`#217` already used to ship
  narrow, individually reviewable, individually verified changes to
  correctness-critical codegen.
* **Skip-conditional adjacency is a hard, first-class hazard, not an
  afterthought.** `banking::assign_banks`'s insertion loop has no
  adjacency guard for "previous line was a skip op" (verified directly
  against `crates/banking/tests/banking.rs:79-87`); it relies entirely on
  `isel`/`legalize`/`alloc` guaranteeing, upstream, that a skip-guarded
  instruction never needs a bank different from what is already tracked.
  `schedule` is the first pass with the power to break that invariant (by
  moving a different-bank instruction into the position right after a skip
  op), so it must treat every `(skip-op, its immediate successor)` pair as
  atomic and untouchable, and this must be enforced by construction, not
  discovered by a failing test.

## Alternatives rejected

* **A module inside `isel`, restructuring emission to build a structured
  instruction list before stringifying.** Would avoid re-deriving
  operand/def-use metadata from text, but requires retrofitting explicit
  metadata onto roughly 6600 lines of ad hoc `self.emit(format!(...))`
  call sites, and collapses the diffable isel/schedule boundary this repo
  treats as load-bearing. A much larger, riskier change for the same
  phase-1 payoff.
* **A module inside `banking`, reordering as part of the existing linear
  scan.** Conflates two different concerns (deciding where `BANKSEL`s go
  vs. deciding instruction order) inside one pass, and `banking`'s own
  `BankSet` dataflow already has real subtlety (`CALL`-exit-bank and
  label-provable-bank joins) that a reordering pass would need to run
  *before*, not interleaved with.
* **General list scheduling from the start.** The 271-candidate count is
  an upper bound, not an expected win; committing to a full cost-model
  scheduler before measuring phase 1's real yield risks a large, hard-to-
  review change built on an unvalidated payoff estimate.
* **Duplicating `banking`'s bank-classification logic in the new crate.**
  Avoids a cross-crate dependency in the "wrong" pipeline direction (a
  minor ergonomic cost), at the price of two independently-maintained
  copies of PIC16F877A's bank arithmetic silently drifting apart, exactly
  the failure mode this investigation exists to prevent.

## Known trade-offs

* **Phase 1's measured win may be zero net flash words even if real.**
  `crates/asm/src/lib.rs`'s `.align N` rounds the running address strictly
  upward; any saving that does not cross an alignment boundary is silently
  absorbed. This already happened to `#213` (18 label resets eliminated, 0
  net words on `hal-pic16-encoder-full`, fully absorbed by a later
  `.table`'s `.align 256`). Phase 1's success criterion is a real,
  safety-checked, oracle-verified reduction in mid-block bank changes,
  honestly measured against the actual flash-word ladder, not a specific
  word count.
* **Only a fraction of the 271 candidate excursions will actually pass
  every phase-1 safety gate.** The real yield is unknown until measured;
  this ADR commits to phase 1's narrow scope specifically so that number
  gets measured cheaply before any larger scheduler is designed.

## Revisit if

* Phase 1's measured results (switch-count and flash-word deltas, reported
  on epic-cc#210) show enough real, safely-reorderable headroom to justify
  multi-instruction excursions, flag-chain reasoning, or cross-`CALL`
  scheduling: scope those as an explicit phase 2 follow-up, not a
  retroactive expansion of this ADR.
* `isel` ever grows a genuine structured instruction representation for
  unrelated reasons (e.g. a future optimization needing def-use chains) --
  re-evaluate whether `schedule` should move onto that representation
  instead of its own text-based `Insn` model, to avoid two independent
  instruction abstractions.
