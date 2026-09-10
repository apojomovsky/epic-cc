# 35: SDCC parity epic

> **Approval status:** design approved by the user on 2026-09-06. This
> document is the design of record for the SDCC parity epic (tracking
> issue, epic-cc). The implementation plans derive from it and do not
> exist yet.

**Goal:** for every target epic-cc supports, be at least as good as
SDCC: same or better language surface, same or better code size, same
or better speed, verified objectively against SDCC's own output. SDCC's
PIC ports are the unmaintained incumbent (manual 1.1: "Microchip PIC is
currently unmaintained"); SDCC itself is actively released and funded.
The bar is "cover every SDCC capability", never "match SDCC's feature
set" (we do not regress to SDCC's limitations).

**Definition of done (epic-level):** all three family sub-epics green,
and the epic-cc vs SDCC comparison is a CI artifact that fails on
regression. Nothing in this epic disassembles SDCC output; every
comparison is black-box.

---

## 1. Why the bar is breadth, not quality

SDCC's strength is surface breadth, not codegen. Its pic16 (PIC18) port
is "not yet mature and still lacks many features" (SDCC manual 4.10),
unmaintained, and has **no regression gate** (manual 4.10.20.2: "no
automatic regression tests are currently performed for the PIC16
target"). Its pic14 port is the same in effect: the manual (4.9.9.2)
says the full regression suite "does not pass, indicating that there
are still major bugs in the port". So "at least as good as SDCC" is a
low bar on code size and cycles and a real bar on language surface:
C99/C11, bit-fields, unions, malloc, math.h, recursion, priority
interrupts. (64-bit ints are *not* part of that bar: SDCC's pic ports
have no or only incomplete support for them — see the gap inventory
below.)

epic-cc is already ahead of SDCC in three places, and the DoD must not
regress them:

- **Struct/union as function parameter and return value.** SDCC's
  pic14 and pic16 ports cannot do either (manual 3.1.1, repeated in
  3.1.5). epic-cc does, via `sret`/`byval`, on both cores.
- **libc.** SDCC pic14 has no libc (manual 4.9.8). epic-cc ships a
  freestanding subset (ADR-018).
- **Varargs on PIC14.** SDCC pic14 does not support variable argument
  lists at all (manual 4.9.9.1: "Functions with variable argument lists
  (like printf) are not yet supported"). epic-cc's PIC14 backend
  supports them (see the PIC14 gap table).

## 2. Licensing: ideas yes, code no

SDCC is GPL. It enters the epic in two roles, both consistent with the
existing GPL boundary in AGENTS.md (GPL tools are external-process-only,
never linked or committed):

- **Oracle.** SDCC compiles the same source; we diff observable
  behavior. It lives in the test image, never in the MIT repo.
- **Design reference.** We read SDCC's source for *ideas*: algorithms,
  techniques, data structures, design patterns. Copyright protects
  expression, not ideas, so reimplementing an idea in Rust is legal.

**The line.** Translating SDCC's C into Rust is a derivative work of
GPL code, and the Rust result is GPL. Writing in Rust does not make it
clean. Never copy code text, structure, control flow, variable names,
or comments wholesale.

**The process that keeps us out of the gray zone:**

1. Read SDCC source, write design notes in our own words (the
   algorithm, the tradeoffs, why it works). The notes are ours.
2. Implement from the notes, not from the source. Close the source
   when writing.
3. Never diff our code against SDCC's. No line-by-line porting.
4. Document provenance in the ADR when a technique is genuinely SDCC's:
   "informed by SDCC's approach to X, reimplemented independently."

**One caveat specific to SDCC.** The device libraries under
`device/non-free/` (pic14/pic16 headers and libs) are **not GPL**; they
carry a separate, Microchip-derived license, and SDCC will not compile
anything for a PIC target without `--use-non-free` (manual 1.2). epic-cc
itself does not need them: epic-cc generates device data from
Microchip's own DFP (ATDF). The **oracle** does need them, unavoidably
— running SDCC at all requires `--use-non-free` and pulls Microchip's
own headers/libs into the build. Decision: the non-free device
libraries and the GPL SDCC binary exist only inside the CI test image,
are never committed to the MIT repo, and are never part of a release
artifact (see the oracle infrastructure section for the image-layering
consequence of this). The rule for *code we write* is: borrow ideas
from the compiler source (GPL, ideas-only), never from the non-free
device libraries.

**The alternative that changes the calculus.** Relicensing epic-cc
under GPL would allow copying SDCC code outright. Rejected: MIT is more
permissive and the project is MIT. Stated so the decision is explicit,
not accidental.

## 3. Gap inventory

Verified against the SDCC 4.6.0 manual (as shipped in the pinned source
tarball — section numbers below are from that revision, not necessarily
whatever is currently posted at sdcc.sourceforge.net, since numbering
has shifted across releases), `pic14devices.txt`, `supported-devices.ac`,
and epic-cc's tree. Items marked **verify** are ones where the corpus
(P0) is the arbiter: clang's frontend may already close them untested.

### PIC18 (SDCC "pic16" port): epic-cc gaps

| Capability | SDCC | epic-cc | Note |
|---|---|---|---|
| C89 core, 8/16/32-bit ints, float, pointers, arrays, structs, varargs, switch, function pointers, inline asm | ✅ | ✅ | parity already |
| Config words | ✅ (`#pragma config`) | ✅ (`EPIC_CONFIG` macro) | different syntax, same capability; epic-cc's is ADR-012; SDCC also needs a device-specific `_ENHCPU_OFF_4L` vs `_XINST_OFF_4L` config name (manual 4.10.20.1) — matters for the corpus harness, see section 4 |
| 64-bit `long long` | ⚠️ incomplete (manual 1.1, footnote 1: "incomplete support in the pic14 and pic16 backends") | partial | the `i64` probe (a 64-bit add with a volatile operand, folded to a byte) passes differentially on PIC18 and computes the hand value on PIC14/PIC14E, where SDCC has no 64-bit type at all (arbitrated); full i64 lowering stays backend work |
| `double` | ✅ as an alias for 4-byte `float`, with a warning emitted (manual 3.1.1/3.1.5: "float is substituted for (long) double"); no `--double` flag exists | ✅ same (f32 only) | parity already, not a gap |
| Bit-fields | ✅ | ✅ | verified: clang lowers to shift/mask IR; `bitfields` probe passes on all three cores |
| Unions | ✅ | ✅ | verified: `unions` probe passes on all three cores (#300) |
| Recursion / reentrancy (FSR1/FSR2 software stack, manual 4.10.12) | ✅ | ✅ | `recursion` probe (fact(5)) passes on all three cores; SDCC's pic14-family static overlay corrupts (arbitrated) |
| Memory models — code-pointer width (manual 4.10.11) | small/large | static overlay only | separate axis from the stack model above |
| malloc / heap | ✅ | ❌ | library work |
| math.h | ✅ | ❌ | library work |
| `printf` `%f` | ✅, but only if `device/lib/pic16` is rebuilt with `--enable-floats` (manual 4.10.9); default build prints `<NO FLOAT>` | ✅ | formatter shipped (#295); the `printf-f` probe formats 3.5 through printf on all three cores (conformance pinned); the p18f4550 differential is arbitrated (SDCC libc never delivers printf output to the portable putchar sink on any stream route, gpsim-confirmed in #352; `sdcc-known-bugs.toml`) |
| Code/eeprom pointers (3-byte generic) | ✅ | partial | pointer-to-const traversal in flash verified at parity on all three cores (`constptr` probe, TBLRD/RETLW path); SDCC's space-qualified pointers (`__code`/`__eeprom`, runtime-selected spaces) stay open under #267 |
| Two-vector priority interrupts | ✅ | ❌ | single-vector compat only (ADR-013 follow-up) |
| `__shadowregs`, `__wparam` | ✅ | ❌ | perf features |
| Real diagnostics vs panics | ✅ | ❌ | known gap |
| EEPROM access | ✅ | ✅ | verified: family-pinned probes write and read back through the EEADR/EEDATA/EECON1/EECON2 window on all three families (p18f4550 differential arbitrated as an SDCC inttoptr limitation) |

### PIC14 (SDCC "pic14" port): epic-cc gaps

| Capability | SDCC | epic-cc | Note |
|---|---|---|---|
| 16F877A / 16F887 | ✅ | ✅ | parity already |
| C89 core, ints, float, interrupts, inline asm, SFR headers | ✅ | ✅ | parity already |
| Struct/union as param/return | ❌ | ✅ | **epic-cc ahead** |
| libc | ❌ | ✅ | **epic-cc ahead** |
| Varargs (`printf` et al.) | ❌ (manual 4.9.9.1) | ✅ | **epic-cc ahead** |
| Bit-fields | ✅ | ✅ | verified: `bitfields` probe passes (#300) |
| Unions | ✅ | ✅ | verified: `unions` probe passes, locals included (#300) |
| 64-bit | ❌ (manual 3.1.3: "pic14: there is no support for 64 bit integer types") | ❌ | no gap |
| math.h | ✅ (`libm.lib`, manual 4.9.8.1) | ❌ | library work |
| Enhanced core (16F193x) | experimental (`libsdcce`) | in progress | the PIC14E sub-epic |

PIC18-table rows not listed here (recursion/reentrancy, malloc,
code/eeprom pointers, two-vector interrupts, `__shadowregs`/`__wparam`,
diagnostics, EEPROM, memory models) are deliberately out of scope for
PIC14: SDCC's pic14 port itself doesn't offer most of them, and where it
does (bit-fields, unions) they're already tracked above.

### PIC14E: the whole core

SDCC: experimental support (16F193x, 12F1822, separate `libsdcce`;
auto-selects the `libsdcc` variant but not `libm`, per manual 4.9.8.1 —
matters if the PIC14E corpus needs math.h). epic-cc: the port is **in
progress** (docs/33, issue #228). P1 (asm encoder + sim core) and P2
(integer spine + BSR/MOVLB banking) have landed; P3 (pointers/arrays/
structs, #272), P4 (const in flash, #278), P5 (interrupts, #283), P6
(32-bit long and mul/div, #284), P7 (soft-float, #297) and P8 (fuzz
in #302. The port is done; what remains is the SDCC parity surface on that core. The
comparison device for this sub-epic is **16F1938** (matches epic-cc's
existing PIC14E device TOMLs and test fixtures).

## 4. The oracle infrastructure (P0)

**Why SDCC and not just XC8.** AGENTS.md already establishes XC8 as a
black-box differential oracle (never disassembled). SDCC is a second,
different oracle, not a duplicate: XC8 is the codegen-quality reference
but license-restricted (fine as an external process, not for anything
we'd want to inspect or vendor); SDCC is the language-surface-breadth
oracle, GPL (ideas-only, per section 2), and freely rebuildable from
source. The two answer different questions and both stay black-box.

**SDCC from source, digest-pinned, in the image**: follows the existing
gputils/XC8 pattern (`PIC8_XC8_ROOT`-style env var, same invocation
conventions — this harness should add `PIC8_SDCC_ROOT`, not invent a
parallel scheme):

- SDCC **4.6.0** (latest release, June 2026) source tarball, sha256
  pinned, built with `--use-non-free` enabled (required for any PIC
  target: manual 1.2) and gputils on `PATH` (gputils 1.5.2 is already
  built earlier in the Dockerfile; manual 2.4.1 confirms pic14/pic16
  need gputils; **verify** 1.5.2 is what SDCC 4.6.0's pic ports expect).
- Build deps to add to `base`: `libboost-dev`, `bison`, `flex`.
- **Image layering.** SDCC itself (GPL) and the non-free Microchip
  device libraries it pulls in must not end up in the `release` image.
  The Dockerfile's stage graph is `base -> clang-builder -> dev ->
  {ci, release}`; build deps may go in `base`, but the SDCC build and
  the non-free tree live in a stage only `ci` inherits (e.g. a
  dedicated `sdcc-builder` stage `COPY --from`'d into `ci` alone), never
  in a stage `release` is built from. The registry build cache must not
  publish those layers either, since `apojomovsky/epic-cc` is a public
  repo.
- The pic16 device library must be rebuilt with `--enable-floats`
  (`device/lib/pic16`, manual 4.10.9): the default SDCC pic16 build
  prints `<NO FLOAT>` instead of formatting `%f`, which would make the
  `%f` Tier-2 program untestable against SDCC.
- SDCC's regression suite vendored into the image (GPL, never
  committed). Note there are two independent suites: the generic one
  under `sdcc/support/regression` and a separate pic14 suite under
  `sdcc/src/regression` (manual 7.8) — Tier 3 below only uses the
  former for cross-compiler comparison.
- gpsim as an optional fallback if our sim cannot run SDCC output —
  but if that fallback ever fires, treat it as a P0 blocker to fix, not
  a permanent routing-around: cycle counts from two different
  simulators are not comparable (see the comparison protocol below).
  Our sim already has Pic14+Pic18 cores and parses Intel HEX, and
  gplink emits standard HEX, so it should just work for the common
  case.

**The harness** (a new crate or script, mirroring the fuzz
differential):

- For each corpus program: compile with epic-cc to hex; compile with
  `sdcc --use-non-free -mpic16 -p18f4550` (or `-mpic14 -p16f877a`)
  through gplink to hex; load both into our sim and run both to the
  program's `sleep` halt (see the corpus contract below), with "budget
  exhausted" a distinct failure class from a value mismatch; then
  compare the program's declared output globals and ports, matched by
  name across the two compilers' symbol/map output, plus the cycle
  count; record flash words + RAM bytes from both. The comparison is
  scoped to named outputs, never the whole RAM image: the two
  compilers allocate globals and locals at different addresses with
  different overlay strategies, so unrelated bytes would differ. This
  mirrors the fuzz differential (crates/fuzz), which compares a single
  named checksum global rather than full RAM.
- **Corpus portability contract.** The corpus is smaller than the
  sketch above, in the direction of less machinery: every corpus
  program is a single plain-C source that compiles unmodified under
  both compilers (no `corpus_compat.h`). Inputs are self-seeding,
  assigned at the top of `main`, because SDCC's PIC18 crt0 clears all
  of BSS before `main` and would wipe any value written into RAM ahead
  of the run. Every program ends in an explicit `__asm__("sleep")`, the
  simulator's halt condition on every core, so both sides' cycle counts
  are measured to the same halt instead of to a step budget (SDCC's
  linked output otherwise loops forever after `main` returns).
- **Comparison protocol**, pinned so "epic-cc <= SDCC" is gradable
  rather than arguable:
  - A fixed, documented flag set per compiler per family (e.g. SDCC's
    `--opt-code-size`/`--opt-code-speed`, `--obanksel`, peephole and
    pstack-model choices; epic-cc's equivalent), recorded alongside the
    version pins.
  - Flash words / RAM bytes / cycles are defined identically for both
    compilers. Recorded choices (implemented in the P0 harness):
    flash = the extent of program-region data records in each side's
    final Intel HEX (config words, ID locations and EEPROM data sit
    above the program region on every supported device, so they are
    excluded without special cases); RAM = live (non-zero) RAM bytes at
    the `sleep` halt, the only definition computable identically for
    two allocators that place everything differently; cycles = both
    compilers' outputs run in *our* simulator to the program's `sleep`
    halt, never to a step budget (a budget exhaustion is its own
    failure class, never a measurement). Mixed-simulator cycle
    comparisons are not valid data points.
  - SDCC is a black-box oracle, but arbitration is not guesswork: every
    known-bug entry is cross-checked under a second, independent
    simulator (gpsim, external process) so the wrong side is proven to
    be the oracle, not our harness.
- Output: per-program table (flash, RAM, cycles, pass/fail) + aggregate
  ratios, published to the CI step summary, with SDCC/gputils/epic-cc
  versions stamped into every output. The ratio gate reuses the driver's
  size-regression mechanism: a committed `crates/sdcc-parity/
  baseline.toml`, fail-on-regression (exact integer cross-multiplication
  of the epic-cc/SDCC ratio), explicit `UPDATE_SDCC_BASELINE=1`
  re-baselining via the `regression_gate` test.

**The corpus** (committed, MIT-clean, ours):

- Tier 1: the existing e2e fixtures (61 on PIC14, 15+ on PIC18). Like
  Tier 2, each fixture needs a recorded expected result so a
  differential mismatch has an arbiter (see DoD item 2 below) — not all
  61+15 currently have one; closing that gap is part of P0.
- Tier 2: one program per SDCC capability: bit-fields, unions, 64-bit
  (conformance-only against epic-cc's own expected value, since SDCC's
  own 64-bit support is incomplete on these ports — no differential),
  malloc, math, code/eeprom pointers, priority interrupts, recursion,
  `%f`, each with a hand-computed expected result. (`double` is dropped
  from this list: SDCC aliases it to 4-byte `float` too, so there is no
  double-vs-float gap to exercise — see the gap inventory.)
- Tier 3 (image-only, **not on the P0 critical path**): SDCC's own
  regression suite, run under SDCC to establish SDCC's own pass rate as
  context. Adapting that suite's harness (`testfwk.h`, its Python
  driver, per-port simulator glue) to run *under epic-cc* is a
  separately-scoped stretch goal, tracked on its own ticket rather than
  blocking the four sub-epics that depend on P0.

## 5. Definition of done (per family sub-epic)

All five must hold:

1. **Conformance.** Every Tier-2 corpus program compiles under epic-cc
   and sim-verifies to its hand-computed result.
2. **Differential.** For every corpus program SDCC accepts, epic-cc's
   sim-observed final state (the program's named output globals and
   ports, matched by name) matches SDCC's, with both programs fed the
   same source (inputs are part of the source, see section 4). Cycle
   count is not part of this item: two compilers emit
   different instruction sequences, so an exact cycle match is
   unachievable and would contradict item 4. Cycle count belongs solely
   to the <= comparison in item 4.

   **Arbitration.** SDCC is a cross-check, not the specification —
   section 1 establishes SDCC's own pic ports have major, acknowledged
   bugs, so a mismatch does not by itself mean epic-cc is wrong. Every
   corpus program (Tier 1 and Tier 2 alike) carries a hand-computed
   expected result; that value is the arbiter. On a mismatch: if
   epic-cc matches the expected value and SDCC does not, the case is
   recorded in a committed `sdcc-known-bugs.toml` (program, SDCC
   version, observed vs. expected, upstream bug link if filed) and
   excluded from the differential gate. If epic-cc does not match the
   expected value, it's an epic-cc bug regardless of what SDCC did.
   Item 2 is green when every non-excluded program agrees with SDCC.
3. **Size.** On every corpus program **that SDCC accepts**, epic-cc
   flash words <= SDCC flash words and RAM bytes <= SDCC RAM bytes,
   under the pinned comparison protocol (section 4). Terminal gate:
   geometric-mean flash ratio <= 1.00 across the accepted corpus, no
   single program worse than 1.10x, and any program above 1.00x carries
   a one-line written rationale in the comparison table (SDCC's
   hand-tuned runtime routines for e.g. multiply/divide may simply be
   better on a handful of programs indefinitely — the aggregate-plus-
   bounded-exception shape is what makes that acceptable without making
   the gate meaningless). CI fails on any *regression* against the
   committed baseline regardless of where the aggregate sits, using the
   same mechanism as `crates/driver/tests/size_regression_e2e.rs`
   (`size_baseline.toml`, fail-on-growth, explicit
   `UPDATE_SIZE_BASELINE=1` re-baselining).
4. **Speed.** Same shape as item 3, for cycle count: on every corpus
   program SDCC accepts, geometric-mean cycle ratio <= 1.00, no single
   program worse than 1.10x, exceptions documented, regressions fail CI.
5. **Surface.** Every SDCC capability on that target is either
   implemented with a Tier-2 test, or listed in the design as a
   deliberate non-goal with a written rationale.

**Epic-level DoD:** all three family sub-epics green, and the
comparison table is a CI artifact that fails on regression. Nothing
disassembles SDCC output; every item is black-box.

## 6. Epic / sub-epic structure on the board

epic-tasks has no native epic type and should not grow one. The
established pattern (epic-hal#59, a tracking issue with the work
decomposed into linked sub-issues) is tracking issues + labels +
blockedBy:

- **Umbrella tracking issue** "SDCC parity epic" (epic-cc).
- **Sub-epic tracking issues**: "PIC18 SDCC parity", "PIC14 SDCC
  parity", "PIC14E port + parity", "SDCC oracle infrastructure".
- **Labels**: `epic:sdcc-parity` on everything; `family:pic14` /
  `family:pic18` / `family:pic14e` per item.
- **Edges**: every work item `blockedBy` its sub-epic; every sub-epic
  `blockedBy` the infrastructure issue. `area:*` labels keep agents
  from colliding as today.
- **No epic-tasks code changes.** Phases stay data; grouping is labels.

## 7. Processes

1. **Baseline first.** P0 runs the full comparison before any gap work.
   The honest starting table is the anchor for "when we're done". Some
   inventory rows marked **verify** may already be green (clang's
   frontend does real work); the baseline decides.
2. **One issue per gap**, DoD = its Tier-2 test passing + differential
   clean. Correctness gaps (SDCC can, we can't) before size/cycle gaps
   (we can, but worse).
3. **Review gate unchanged**: develop, separate reviewer, takeoff, PR.
4. **CI**: nightly full comparison (both compilers, all tiers, versions
   recorded in the output); PR runs the committed corpus only. A ratio
   regression fails CI.
5. **Pinning**: SDCC 4.6.0 + gputils versions recorded in every
   comparison output, so the baseline is reproducible. Pinned by sha256
   specifically because upstream may prune the pic trees later (SDCC's
   own feature backlog includes a request to remove the pic14/pic16
   autogenerated headers and libs from the source tree); the oracle
   tracks the pinned 4.6.0 tarball, never SDCC head.

## 8. Sequencing

- **P0**: SDCC oracle + harness + baseline (blocks everything).
- **P1**: PIC18 parity (the stated priority; biggest surface).
- **P2**: PIC14 parity (small; three axes already won).
- **P3**: PIC14E port + parity (P1-P2 landed; finish P3-P8, then
  parity).

## 9. Where SDCC's ideas help most

The process rule (section 2) slots into each gap ticket's design
phase: read SDCC for the idea, write our own design notes, implement
from the notes. The highest-value targets:

- **Recursion / frame model** (PIC18): SDCC's pic16 port has a real
  FSR1/FSR2 stack model. Our `Slot::Frame` hook (docs/29 D-2) is the
  same idea; reading SDCC informs the design directly.
- **64-bit `long long`**: SDCC's pic ports have incomplete/no support
  for it, so they're a weak reference here; SDCC's non-pic ports (e.g.
  z80, stm8) are the more useful design reference for the runtime
  routines and lowering approach, not the code.
- **malloc / math.h**: SDCC's library design (heap structure, math
  routines) is a legitimate design reference.
- **Bit-fields, unions, `%f`**: mostly clang-frontend work, but SDCC's
  lowering choices are worth reading.
- **Bank-selection minimization**: SDCC has real work here; our
  linear-tracking gap (README) is exactly where their technique is
  informative.
