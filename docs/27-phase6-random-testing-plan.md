# Integer Spine — Milestone 14: Random Testing at Scale Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** phase 6's first half — an unsupervised differential fuzz loop (YARPGen-style generation + cvise-style reduction). A seeded generator emits C programs in the supported surface; each is compiled **twice** — by our driver (→ `pic14-sim`) and by **host clang** (→ a native run) — and the two checksums must match. A mismatch or a compiler panic (the loud-panic contract) is a bug: a greedy reducer minimizes it to a reproducer, saved as a fixture. Acceptance: a fixed-seed corpus (e.g. 200 seeds) runs differential-clean, and the harness catches at least one real bug (or, if the surface is already clean, the corpus documents that).

**Architecture:** a new `crates/fuzz` crate: the seeded generator (deterministic RNG), the differential runner (driver → hex → sim vs host clang → native), and the reducer (greedy statement/expression deletion). The generated C is **unsigned-only and layout-agnostic** (explicit-width types, no signed overflow, shifts < width, nonzero divisors, field-wise struct access) so host and PIC semantics coincide; volatile input globals are seeded identically on both sides (the sim via the alloc map, the host via a generated `host_main.c`).

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned — also the host compiler), `pic14-sim`, gpasm (unused here), the existing driver + e2e harness patterns.

## Global Constraints

- Build/test with `nix develop --command cargo …`; never bare `cargo`.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`) for the PIC side; **host clang** (no `-target`) for the reference.
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; gpasm external-process test-only.
- Unsupported constructs panic loudly, never silently miscompile.

## The design (load-bearing)

### The generator's C discipline (host/PIC semantic equivalence)

- **Types**: `unsigned char` / `unsigned short` / `unsigned long` ONLY (explicit widths — never plain `int`/`long`: msp430's `int` is 16 bits, the host's is 32). Casts are explicit narrowing/`(unsigned long)` widening — defined on both.
- **Arithmetic**: unsigned only (wrapping is defined). No signed overflow (UB on the host).
- **Shifts**: counts always `< width` (constants < width, or `(x & (width-1))`).
- **Division**: divisors always nonzero (nonzero constants, or `d | 1`).
- **Control flow**: `if`/loops with unsigned conditions; loops bounded (a small trip count).
- **Arrays/pointers**: small `unsigned char` arrays, dynamic index `arr[i]` with `i % N` (in-bounds by construction); reads/writes; no pointer arithmetic beyond array indexing.
- **Structs**: simple (a few `unsigned char`/`unsigned short`/`unsigned long` fields), field-wise access ONLY (no layout-dependent constructs — the host's 32-bit alignment differs from the PIC's).
- **No**: signed ops, unions, recursion, function pointers, global initializers relying on layout.
- **The checksum**: `checksum = (unsigned char)(checksum * 7 + v)`-style folds over the computed values (well-defined on both), ending in `volatile unsigned char checksum;` written from `main`.
- **Volatile inputs**: the generator emits `volatile unsigned char in0; volatile unsigned short in1; …` (zero-init, uninitialized values) — the HARNESS seeds them identically on both sides.

### The differential runner

For each seed:
1. Generate `prog.c` + the input-value list + the globals metadata (name → the sim-side address comes from the driver's alloc layout at run time).
2. **PIC side**: the driver (`PIC8_CLANG_UNWRAPPED`/`PIC8_CLANG_RESOURCE_DIR` env) → hex → `pic14-sim`: seed the volatile inputs' RAM at their alloc'd addresses (the e2e pattern), run, read `checksum` from its address, and `halted()` must be true.
3. **Host side**: host clang compiles `prog.c` + a generated `host_main.c` (renames the generated `main` via `-Dmain=pic_main`; seeds the input globals by name; calls `pic_main`; prints `checksum`), runs it, reads the printed value.
4. Compare. A mismatch, a driver/isel/asm panic, an assembler failure, or a non-halting sim run = a FAILURE (feed it to the reducer).

### The reducer (cvise-style greedy)

On a failure: iterate over the generated program's statements (the generator emits a linear, structurally-known `main`), try deleting each statement/expression (or replacing an expression with a constant/one of its operands), re-run the differential, and keep the deletion when the failure persists. Stop at a fixed-point; save the minimal `reduced_<seed>.c` + the failure report as a fixture. The reduction must preserve the failure (same differential check).

### The harness shape

`crates/fuzz` (lib + a `main.rs` runner + tests):
- `generate(seed) -> Program { c_source, inputs: Vec<(name, u8/u16/u32, width)>, checksum_name }`.
- `run_differential(program) -> Result<u32 /*checksum*/, Failure>` (PIC side + host side).
- `reduce(program, failure) -> ReducedProgram`.
- The committed test: `cargo test -p fuzz` runs a fixed small seed set (e.g. 8 seeds, fast); `cargo test -p fuzz -- --ignored` (or a `--release` run) runs the full corpus (e.g. 200 seeds). Deterministic (seeded RNG, no wall-clock dependence).

---

### Task 1: the generator + the differential harness skeleton

**Files:**
- Create: `crates/fuzz/Cargo.toml`, `crates/fuzz/src/lib.rs`, `crates/fuzz/src/main.rs` (or test binaries), `crates/fuzz/tests/differential.rs`
- Modify: the workspace `Cargo.toml` (add the member)

**Interfaces:**
- Produces: `generate(seed)` (the C discipline above; the generator's statements structurally trackable for the reducer); `run_differential` (PIC: driver + sim seeded from the alloc map; host: host clang + host_main.c); a `checksum_eq` comparison.

- [ ] **Step 1: Write the failing tests** — a hand-written tiny program (`volatile unsigned char in0; volatile unsigned char checksum; void main(void){ checksum = (unsigned char)(in0 * 7 + 3); }`) — the differential returns the same checksum on both sides for a few seeds (in0 = 0, 1, 200); a mismatching variant FAILS the comparison.
- [ ] **Step 2: Run to verify they fail** (no harness).
- [ ] **Step 3: Implement** — the generator (seeded RNG: `SplitMix64` or `rand_pcg` — prefer a small self-contained LCG to avoid new deps; document) + the runner (the driver invocation mirrors the e2e; the sim seeding via the alloc map; the host side: `clang -O1 -Dmain=pic_main prog.c host_main.c -o prog && ./prog`).
- [ ] **Step 4: Run to verify they pass** + a manual 20-seed smoke (no panics; if a real bug appears, note it — the milestone's value — and fix or document).
- [ ] **Step 5: Commit** — `git commit -m "feat(fuzz): seeded generator and differential harness"`.

---

### Task 2: the generator surface + the corpus

**Files:**
- Modify: `crates/fuzz/src/lib.rs`
- Test: `crates/fuzz/tests/differential.rs` (extend)

**Interfaces:**
- Produces: the generator's full surface: scalar arithmetic (+ - * / % & | ^ << >> on u8/u16/u32), comparisons (< <= > >= == !=), if/else + bounded loops, noinline calls (0–3 unsigned params), arrays with dynamic indices, structs (field-wise), the checksum fold. The committed corpus: 200 fixed seeds (deterministic) — run in `--ignored` mode; a fast 8-seed subset in the normal test.

- [ ] **Step 1: Extend the failing tests** — a corpus seed that exercises each construct (a seed known to generate calls, one with loops, one with structs/arrays) — all differential-clean.
- [ ] **Step 2: Run to verify they fail** (the surface not yet generated).
- [ ] **Step 3: Implement** — the surface; the corpus seeds; document any REAL BUGS the fuzz finds (the milestone's value — fix the compiler bugs and keep the seeds, or document them if out of scope).
- [ ] **Step 4: Run to verify they pass** — the fast subset green; the full 200-seed run clean (or the found bugs fixed + re-run).
- [ ] **Step 5: Commit** — `git commit -m "feat(fuzz): full generation surface and seed corpus"`.

---

### Task 3: the reducer

**Files:**
- Modify: `crates/fuzz/src/lib.rs`
- Test: `crates/fuzz/tests/reduce.rs` (extend)

**Interfaces:**
- Produces: `reduce(program, failure)` — greedy statement/expression deletion while the failure persists (the generator's structural knowledge); the minimal reproducer saved as `reduced_<seed>.c`; the reduction preserves the failure.

- [ ] **Step 1: Extend the failing tests** — a synthetic mismatching program (e.g. one statement that flips the checksum): the reducer removes the OTHER statements and keeps the culprit; the reduced program still fails the differential.
- [ ] **Step 2: Run to verify they fail** (no reducer).
- [ ] **Step 3: Implement** — the greedy loop (statement deletion + expression replacement with a constant/operand; the failure check re-runs the differential); a fixed-point + a reduction cap (e.g. 5000 re-runs).
- [ ] **Step 4: Run to verify they pass** — the reducer tests green; run it on one real corpus failure if any were found in Task 2 (else on the synthetic case).
- [ ] **Step 5: Commit** — `git commit -m "feat(fuzz): greedy differential reducer"`.

---

### Task 4: Acceptance — the corpus run + the reduced-bug fixtures

**Files:**
- Modify: `crates/fuzz/tests/differential.rs`, `crates/fuzz/tests/reduce.rs`
- Create: `crates/fuzz/fixtures/reduced_*.c` (any real bugs found)

**Interfaces:**
- Consumes: Tasks 1–3. Produces: the committed 200-seed corpus run is clean (or the found bugs are fixed with their reduced reproducers committed as regression fixtures); the harness's panic-catching is exercised (a deliberately-broken seed panics the driver → the differential reports it as a failure, the reducer minimizes it).

- [ ] **Step 1: Write the failing acceptance** — run the full corpus (200 seeds) via the ignored test; the reduced reproducers (if any bugs were found) are committed as fixtures with the bug fixed in the compiler (the fix + the fixture = the milestone's regression evidence).
- [ ] **Step 2: Run to verify it fails** (any unfixed bug).
- [ ] **Step 3: Fix the found bugs** (each: the reduced reproducer → the responsible stage → the fix → the fixture test asserts the differential now passes) — or document any out-of-scope findings.
- [ ] **Step 4: Run the full suite** — workspace green (probe → long → interrupt → fuzz corpus).
- [ ] **Step 5: Commit** — `git commit -m "test(fuzz): differential corpus and reduced reproducers"`.

---

## Self-review notes

- **Spec coverage:** M14 delivers phase 6's unsupervised differential loop (generation + differential + reduction). The full phase-6 ambition (YARPGen-style deep IR-level fuzzing, cvise integration) is approximated with the built-in generator/reducer — the architecture leaves room to swap in external tools later.
- **Correctness risks:** (1) the host/PIC semantic equivalence is the whole game — the generator's discipline (explicit widths, unsigned-only, guarded shifts/divisors, field-wise struct access) is load-bearing; a discipline violation produces false mismatches; (2) the volatile-input seeding must be identical on both sides (the alloc-map addresses vs the host's by-name seeding); (3) the reducer must preserve the failure (each deletion re-runs the differential).
- **False-positive hygiene:** the committed corpus must be deterministic (seeded RNG, no time/locale dependence); the host clang is the same pinned clang (host target) — a compiler-side difference (e.g. host UB) is avoided by the discipline, not papered over.
- **Deferred (later milestones):** signed-arith fuzzing (needs wrap-safe signed generation), IR-level fuzzing, external cvise integration, soft-float fuzzing.
- **Contract:** the generator's C discipline, the `Program` shape (source + inputs + checksum name), the differential check (checksum + halted), and the reducer's failure-preservation are the crate's contracts.
