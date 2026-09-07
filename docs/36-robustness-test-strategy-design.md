# 36: Robustness test strategy, beyond differential fuzzing

> **Approval status:** approved by the user on 2026-09-07. This
> document is the design of record for
> [issue #273](https://github.com/apojomovsky/epic-cc/issues/273) and
> the six sibling lanes it proposes (tracked as separate issues, see
> section 5). Implementation plans derive from it and do not exist
> yet.

**Goal:** close the class of testing gap that neither the existing
differential fuzz harness (`crates/fuzz`) nor the SDCC parity epic
(`docs/35`) covers: crash-hunting on unstructured or adversarial input,
auditing whether the existing suite would even catch a given bug class,
and correctness properties that a checksum-based oracle cannot see
(determinism, instruction ordering, resource limits). Seven lanes,
each scoped enough to become its own implementation plan once approved.

## 1. What already exists, and the axis this document does not touch

epic-cc's test suite already has, and this document does not propose
to duplicate:

- **Differential fuzzing** (`crates/fuzz`): a seeded generator produces
  small C programs in a host/PIC-equivalent subset, compiled twice
  (our driver plus host clang, or directly at the IR level per
  issue #14), and a mismatched checksum, a compiler panic, or a
  non-halting simulator run is a bug. A greedy reducer minimizes any
  failure to a fixture.
- **Two independent behavioral oracles** (XC8, gpsim) and an assembler
  cross-check (gpasm byte-diff against our own assembler's output),
  per `docs/05-verification.md`.
- **A large e2e fixture set per feature**, per-device sanity checks,
  and CI stratified so a core-wide bug is caught once, not once per
  device.
- **The SDCC parity epic** (`docs/35`): a *different axis*. It
  benchmarks feature surface, code size, and cycle count against SDCC,
  answering "are we at least as good as the unmaintained incumbent."
  It is not a bug-hunting or robustness effort, and nothing in this
  document overlaps with it.

The gap this document addresses: every lane above needs the compiler
to already be handed a plausible-looking, generator- or human-produced
program. None of them throw structurally arbitrary input at the
pipeline. None of them ask whether the *existing* suite would catch a
given class of bug at all. None of them check process properties
(determinism, idempotence) as opposed to output correctness. None of
them isolate the one correctness property that is genuinely
MCU-specific, namely volatile access ordering, from the "final
checksum matches" oracle, which cannot see it. None of them test what
happens at the compiler's own resource limits, as opposed to genuinely
unsupported syntax.

## 2. The seven lanes

### Lane A: GCC c-torture compile-only ICE corpus (issue #273, as filed)

The ticket's own scope stands: pull a filtered slice of
`gcc.c-torture/compile`, run each program through the frontend and
backend, and assert only "does not panic." Two refinements to fold
into that lane's implementation plan:

- **Filter by rule, not by list.** The ticket's own stated risk of a
  "maintenance-heavy skip list" is avoided by a syntactic prefilter
  (reject files containing VLA syntax, `_Complex`, computed-goto
  `&&label`, nested functions, `__int128`, wide `long double`
  literals) rather than a hand-maintained per-file exclusion list that
  needs upkeep as the corpus or the roadmap changes.
- **Minimize any ICE before filing it.** The ticket's text stops at
  "assert does not panic." `crates/fuzz::reduce` is not directly
  reusable here: it takes a `Program` (a checksum, seeded inputs, a
  host twin) and its greedy loop unconditionally re-runs the full
  differential (PIC side and host side) on every candidate, which a
  bare torture-suite source file has neither the shape for nor any use
  for, since Lane A's oracle is "does not panic," not a checksum match.
  What is reusable is the *technique*: the same greedy statement/line
  deletion down to a fixed point, re-checking only "still panics" after
  each deletion instead of re-running a differential. A lightweight,
  host-free variant of that loop, purpose-built for Lane A, should
  minimize any crash the corpus finds, with the reduced reproducer
  committed as a permanent regression fixture. A nightly job that finds
  a crash and only logs it is a fire alarm nobody answers.

Oracle: none needed, "does not panic" is a self-contained check.
CI placement: nightly, non-blocking, exactly as the ticket scopes it.

### Lane B: coverage-guided in-process fuzzing (cargo-fuzz)

Lane A and the differential generator both require the compiler to be
handed a program that already looks like plausible C. Neither one
exercises what happens when a pipeline stage is handed structurally
broken input directly, the way AFL- and libFuzzer-style fuzzing found
the large majority of front-end crashes in GCC and LLVM: not by
generating more plausible programs, but by mutating bytes under
coverage feedback, which is not limited to what a generator's author
imagined.

Target: `irparse`'s parser for the canonical IR dialect already
introduced by the IR-level differential lane (issue #14). It is
in-process, has a well-defined text grammar, and sits directly behind
the `.ll`-to-IR boundary that the whole backend trusts. A second
candidate, once the first is running, is any other stage that parses
untrusted text rather than only IR built in-process.

Corpus seed: there are no standalone `.ll`/IR fixture files in the
tree today; the IR text the workspace already exercises lives as
inline string constants inside test source files (for example
`crates/irparse/tests/parse_ll.rs`, `sanitize.rs`). Seeding the fuzz
corpus needs a small one-time extraction step, pulling those string
constants out into real corpus files, rather than pointing at
pre-existing files that do not exist.

Crash triage: `cargo fuzz tmin` (the tool's own minimizer, since the
input here is IR text, not generated C, so the C-level reducer does
not apply) on any crash, landed as a fixture plus a regression test,
same discipline as Lane A.

CI placement: not the PR or nightly gate at first. Coverage-guided
fuzzing wants sustained run time, not a single pass, so it belongs as
a periodic longer-running job (weekly, or an opt-in `make` target a
developer runs locally), not something the nightly job blocks on
finishing.

### Lane C: mutation testing of the compiler itself (cargo-mutants)

This is the direct instrument for the question this whole exercise is
about: what is the existing suite failing to catch. Mutation testing
injects small semantic mutations into the compiler's own source and
reports which mutants survive, meaning no test failed. A surviving
mutant is evidence of a testing gap, not a compiler bug.

Scope: start with `crates/alloc` and `crates/banking`, the two crates
the project's own framing already names as the highest-risk surface
("the allocator is the compiler"). Widen to other crates only after
this scope proves the process is worth the running time.

Process: run `cargo-mutants`, triage each surviving mutant into either
"write the missing test" (the common case) or "document why this path
is untestable" (dead code, a defensive-only branch), rather than
treating 100% mutation coverage as the goal.

CI placement: not a gate. Mutation testing runs are slow by nature; a
periodic or manual audit whose output is a checklist of concrete test
gaps to file as follow-up issues, not a job in the standard pipeline.

### Lane D: idempotence, fixpoint, and determinism checks

Two related checks that no existing lane covers, because both are
about the compiler's own behavior across repeated runs, not about
whether a given output is correct:

1. **Determinism.** Compiling the same input twice must produce
   byte-identical output. This catches a real bug class (hash-map or
   hash-set iteration order, an unstable sort) that a single-run
   correctness check cannot see, and matters specifically here because
   the entire differential-fuzz methodology assumes determinism to
   begin with.
2. **Fixpoint idempotence.** A pass meant to reach a fixpoint
   (`peephole`, `legalize`) should be a no-op the second time it runs
   on its own output. Running it twice and diffing catches a pass that
   thinks it converged but did not.

Scope: unit-level, inside the existing `crates/peephole` and
`crates/legalize` test suites, plus one e2e-level "compile twice, diff
the hex" wrapper. No new crate needed.

### Lane E: volatile access ordering and count tests

The differential oracle compares a single final checksum. It cannot
see whether two writes to distinct volatile globals or SFRs were
coalesced into one, or reordered relative to each other: both would
still pass a checksum-based comparison on the simulator, while quietly
breaking real hardware register sequencing, which depends on write
order and count, not merely on the final bit pattern.

Design: hand-written, directed tests, not generated ones, since the
property under test is about the instruction stream, not the value
computed. N writes to N distinct volatile globals; assert the emitted
assembly (or the `isel`/`alloc` stage boundary text) contains N stores
in program order. Before writing new fixtures, audit
`runtime_sfr_e2e.rs` and the interrupt e2e tests for whether this
property is already implicitly covered; this lane should close
whatever gap remains, not duplicate existing coverage.

### Lane F: resource-exhaustion boundary tests (the graceful-failure contract)

The existing contract, stated in `docs/05-verification.md` and
`AGENTS.md`, is that unsupported syntax panics loudly rather than
silently miscompiling. A legitimately-oversized program, one that uses
only supported constructs but genuinely does not fit the target's RAM,
call-stack depth, or flash, is a different failure mode: not
unsupported syntax, just too big, and today's behavior at that
boundary is untested.

Design: exact-fit and one-past-fit fixtures per resource axis (RAM,
the hardware call-stack depth, flash) per core family, asserting a
specific, actionable diagnostic message and exit code, not merely
"does not corrupt output." This closes the other half of the contract
that "loud panic on unsupported input" already covers for syntax.

### Lane G: compile-time bound ("does not hang") tests

`crates/banking`'s `BANKSEL` minimization (pipeline stage 8) is the
pass the project's own documents (`docs/01-target-pic14.md`,
`docs/12-backend-design.md`) cite as NP-hard, even with variables
already pre-assigned to banks: it is a dataflow problem over the
control-flow graph's bank state, not a property of the overlay
allocator in `crates/alloc`. A control-flow shape with enough
bank-crossing accesses and branching could regress that dataflow pass
into a combinatorial blowup that nothing today would catch until a
real build hangs.

Design: a small number of adversarially-shaped fixtures (deeply
branching control flow with many distinct-bank accesses interleaved,
sized to be practical to keep in the repository) targeting
`crates/banking` specifically, with a wall-clock timeout assertion in
CI. This is a performance-regression class distinct from every
correctness lane above it, and distinct from Lane F's resource-limit
fixtures, which target size, not running time.

## 3. Named but out of scope for this document

- **EMI-style testing** (mutate profiled-dead statements in an
  already-passing differential program and recompile it with itself,
  expecting unchanged output; Yang et al., PLDI'12, the same lineage
  as C-Reduce). A legitimate low-cost fourth fuzzing angle that needs
  no host-equivalence discipline at all, since it never leaves our own
  compiler. It is an extension of the existing `crates/fuzz` generator
  rather than a new lane, and is worth a follow-up ticket once lanes
  A through C have shown their value, not launch scope here.
- **Stage-boundary snapshot testing** (`insta`). `docs/05-verification.md`
  already calls for this ("a miscompile bisected to a stage before
  anyone reads code"), but no crate uses it today. This is a testing
  infrastructure gap between design and practice, not a bug-finding
  lane, so it does not belong in a document framed around closing
  robustness gaps; flagging it here for whoever picks it up next.
- **Cross-backend differential** (PIC14 vs. PIC18 on the shared C
  subset). A legitimate, essentially free signal once `crates/fuzz`'s
  harness supports a second target, but it depends on extending that
  harness's frame-budget plumbing to two targets at once, so it
  belongs as a `crates/fuzz` follow-up, not its own lane here.
- **Translation validation** (an Alive2-style formal check that a
  peephole rewrite preserves semantics). The natural "next level"
  after this document, but there is no existing formal semantics for
  the PIC14/PIC18 backend to validate against, and building one is an
  epic in its own right, not a lane.
- Anything overlapping the SDCC parity epic's scope (feature surface,
  code size, cycle count against SDCC): a different axis, tracked
  separately, and deliberately not folded in here.

## 4. Priority and sequencing

Lane A first: already filed, already scoped, cheapest to land. Lanes B
and C next, and specifically before D through G, because both audit
the suite itself rather than assuming its shape: B finds gaps by
throwing input the generator would never construct, C finds gaps by
checking whether existing tests would catch a given mutation at all.
Together they are more likely to reveal which of D through G actually
matter in practice than guessing up front. D, E, F, and G are each
self-contained and low blast radius once A through C land, and can
proceed in parallel in any order after that.

Every lane here carries the same priority as issue #273 itself: a
quality and robustness improvement, not a correctness or feature gap.
None of them blocks other work, and nothing is blocked by them.

## 5. Tracking issues

Issue #273 remains the tracking issue for Lane A; its implementation
plan should fold in the reducer-integration note from section 2 above.
Each sibling lane has its own tracking issue, filed alongside this
document and pointing back at it as its design of record: Lane B is
#286, Lane C is #287, Lane D is #288, Lane E is #289, Lane F is #290,
and Lane G is #291. No new board mechanism was needed: this mirrors
the tracking-issue-plus-labels pattern the SDCC parity epic already
uses (`docs/35`, section 6) rather than inventing a second one.

## 6. Definition of done (per lane)

A lane is done when: its harness exists and runs where this document
places it (nightly, periodic, or a unit test, per lane); any real bug
or coverage gap it found while landing has been fixed or filed with a
minimized reproducer, per the same discipline `crates/fuzz` already
uses for differential failures; and its CI placement matches what
section 2 specifies for that lane, so lanes with slow or open-ended
running time (B, C) do not become gates by accident.
