# Integer Spine — Milestone 15: Soft-Float Implementation Plan (phase 7, final roadmap phase)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `float` (IEEE754 single, 32-bit — msp430's `float`/`double` are both f32) compiles. The float surface: `fadd`/`fsub`/`fmul`/`fdiv`, all 14 `fcmp` predicates, the int↔float conversions (`fptoui`/`fptosi`/`uitofp`/`sitofp`), `fpext`/`fptrunc` (no-ops — double == float on msp430), float params/returns/globals/struct members, and f32 constants. Arithmetic lowers to **runtime routines** (`__add_f32`, `__sub_f32`, `__mul_f32`, `__div_f32`, `__uitofp_f32`, `__sitofp_f32`, `__fptoui_f32`, `__fptosi_f32`, `__cmp_f32`) with **round-to-nearest-even** (the IEEE default — matching host clang, so the M14 differential extends to float for independent rounding verification). Acceptance: a float program with hand-computable results (exact values + one RNE-rounded inexact case) runs correctly and our HEX matches gpasm byte-for-byte. This completes the phase-7 roadmap.

**Architecture:** `crates/ir` + `crates/irparse` gain `Ty::F32` (bytes 4, align 2) and parse the float instructions/predicates/constants; `crates/legalize` rewrites every float op to a routine call (fcmp → `__cmp_f32` + a per-predicate icmp/select materialization tree over its tri-state byte) and injects the routines; `crates/isel` emits the routine bodies (the hand-written IEEE754 asm — the milestone's bulk); `crates/fuzz` extends the differential generator to float bit-patterns (the RNE verification against host clang). `pic14-sim` is byte-based (no float work); gpasm assembles the routine asm unchanged.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned — the PIC front end AND the host oracle), `pic14-sim`, gpasm (test oracle).

## Global Constraints

- Build/test with `make exec CMD="cargo ..." …`; never bare `cargo`.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`) for the PIC side; host clang (no `-target`) for the float differential.
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; gpasm external-process test-only.
- Unsupported constructs panic loudly, never silently miscompile.

## The lowering recipes (load-bearing design)

### The float format and the routine conventions

- f32 = 4 bytes, little-endian: b0 = mantissa LSB … b2 = mantissa MSB, b3 = `sign | exponent[7:0]` (bit 31 = sign; bits 30-23 = the biased exponent; bits 22-0 = the fraction with the implicit leading 1). `1.0` = 0x3F800000 (b3=0x3F, b2=0x80).
- The routines take f32 params (4-byte slots) and return f32 (the 4-byte retval region 0x71-0x74); scratch in the injected `__scr` allocas (the M8/M12 pattern); bank-0 ≤ 0x7F asserts.
- **Round-to-nearest-even**: the round bit + sticky (any lower bit set) + the LSB; on a tie (round bit set, sticky clear, LSB clear) round DOWN (keep even). On a rounding carry, renormalize.

### The routines (the algorithms)

- **`__add_f32(a, b)`** — the hardest: extract sign/exp/mant (mant = `(b2 & 0x7F) << 16 | b1 << 8 | b0 | 0x800000`), if the signs differ do a mantissa subtraction (with the sign of the larger), align the smaller exponent's mantissa right by the difference (up to 31, collecting a sticky bit), add/sub the 24-bit mantissas (the carry bumps the exponent), normalize (shift left until bit 23 set, adjusting the exponent), round RNE, handle zero (exp 0) and the sign. `__sub_f32(a, b)` = flip b's sign bit (XOR b3 with 0x80) then the add path.
- **`__mul_f32(a, b)`**: sign = XOR; exp = e1 + e2 − 127 (bias correction); mant = the top 25 bits of the 24×24 product (a 24-bit shift-add multiply, the AN526 pattern extended); normalize + round RNE.
- **`__div_f32(a, b)`**: sign = XOR; exp = e1 − e2 + 127; mant = 24-bit restoring division (the M8 pattern); normalize + round RNE. Div-by-zero → the IEEE infinity (0x7F800000 — deterministic, documented; the acceptance avoids it).
- **`__uitofp_f32(u32)`**: find the leading 1 (shift left until bit 31, counting), build exp = 127 + 31 − shifts, mant = the top 24 bits (+ the guard for rounding), round RNE. `__sitofp_f32(i32)`: abs (unsigned) + uitofp + set the sign bit.
- **`__fptoui_f32(f32)`**: exponent → shift the mantissa right by (127 − e + 23), clamp/truncate; `__fptosi_f32`: sign + fptoui.
- **`__cmp_f32(a, b)`**: returns a tri-state byte `0 = equal, 1 = a < b, 2 = a > b, 3 = unordered (NaN)` — compare the sign/exp/mant (careful with −0 == +0 and the sign-magnitude ordering); NaN (exp 0xFF, mant ≠ 0) → 3.

### The fcmp materialization (legalize)

`%c = call i8 @__cmp_f32(a, b)` then a per-predicate icmp/select tree over `%c` (i8 → i1), e.g.:
- `oeq` = `icmp eq i8 %c, 0`; `one` = `(c==1) || (c==2)` via two icmps + a select; `ugt` = `(c==2) || (c==3)`; `uno` = `icmp eq i8 %c, 3`; `ord` = `icmp ne i8 %c, 3`; `ult` = `(c==1) || (c==3)`; etc. — all 14 predicates are small select/icmp trees on the i8 result (the existing i8 icmp/select machinery; no i1 binops — the isel rejects i1 binops).

### The conversions and casts

- `fptoui`/`fptosi`/`uitofp`/`sitofp` — i8/i16/i32 ↔ f32 → the four conversion routines (the msp430 `int` is i16 — the conversions must handle i8/i16/i32 source/target widths: the routine takes the value in a slot of the operand's width; the i8/i16 cases use the low bytes).
- `fpext`/`fptrunc` (f32→f32 — double == float on msp430) → a plain copy (legalize rewrites to nothing/`freeze`-style).
- f32 constants (`f32 0x3F800000` hex and `f32 1.000000e+00` decimal forms) → `Val::Const` bit patterns (4 bytes via the existing emit_load_byte).

---

### Task 1: `ir` + `irparse` — `Ty::F32` and the float instructions

**Files:**
- Modify: `crates/ir/src/lib.rs`, `crates/irparse/src/lib.rs`
- Test: `crates/ir/tests/roundtrip.rs`, `crates/irparse/tests/parse_ll.rs` (extend)

**Interfaces:**
- Produces: `Ty::F32` (bytes 4); the `Inst` variants for fadd/fsub/fmul/fdiv/fcmp (the 14 predicates)/fptoui/fptosi/uitofp/sitofp/fpext/fptrunc (or a compact `Float` inst family — decide and document); `f32` constant parsing (the hex bit pattern AND the decimal form); the struct layout: f32 fields size 4 align 2; the canonical text for the float insts.

- [ ] **Step 1: Extend the failing tests** — parse_ll the f1.ll shapes (`fadd float %a, %b`, `fcmp olt float %a, %b`, `fptosi float %1 to i16`, `sitofp i16 %1 to float`, `f32 0x3F800000` + a decimal `f32 1.0` constant); roundtrip; the struct layout with an f32 member.
- [ ] **Step 2: Run to verify they fail** (no F32/float insts).
- [ ] **Step 3: Implement** — the Ty + the insts + the constants + the layout arm. (isel still panics on the float insts — Task 3; minimal loud arms; legalize passes them through.)
- [ ] **Step 4: Run to verify they pass** + workspace builds.
- [ ] **Step 5: Commit** — `git commit -m "feat(ir,irparse): float type and instructions"`.

---

### Task 2: `legalize` — the float lowering

**Files:**
- Modify: `crates/legalize/src/lib.rs`
- Test: `crates/legalize/tests/legalize.rs` (extend)

**Interfaces:**
- Produces: fadd/fsub/fmul/fdiv → `__add_f32`/`__sub_f32`/`__mul_f32`/`__div_f32`; fcmp → `__cmp_f32` + the per-predicate icmp/select trees (all 14); the conversions → the four routines; fpext/fptrunc → copies; the routine Funcs injected with scratch allocas (layouts documented as the contract); f32 params/returns through the normal ABI.

- [ ] **Step 1: Extend the failing tests** — legalize a module with the f1.ll shapes: `fadd` → `call float @__add_f32`; `fcmp olt` → `call i8 @__cmp_f32` + the materialization tree (assert the tree's shape for a few predicates: oeq, one, ugt, ord, uno); the injected routine defs; fpext → a copy (no call).
- [ ] **Step 2: Run to verify they fail** (pass-through).
- [ ] **Step 3: Implement** per the recipes (the tree shapes documented).
- [ ] **Step 4: Run to verify they pass** + workspace (isel still panics on the calls — Task 3).
- [ ] **Step 5: Commit** — `git commit -m "feat(legalize): lower float ops to runtime calls"`.

---

### Task 3: `isel` — the soft-float routine bodies

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: the nine routine bodies (`__add_f32`, `__sub_f32`, `__mul_f32`, `__div_f32`, `__uitofp_f32`, `__sitofp_f32`, `__fptoui_f32`, `__fptosi_f32`, `__cmp_f32`) — the hand-written IEEE754 asm per the recipes (RNE rounding); scratch layouts per the Task-2 contract; bank-0 ≤ 0x7F asserts.

- [ ] **Step 1: Extend the failing tests** — panic-first on the 9 names (the M8/M12 pattern), then the recipes. **SIM (load-bearing) with a Rust f32 reference**: for each routine, fixed-input modules assembled + run with the expected values computed by Rust's f32 arithmetic (RNE — must match bit-for-bit): add (0.5+0.25=0.75; 1.0+1.0=2.0; 2.0−1.0=1.0 via __sub_f32; 0.1+0.2=0x3E4CCCCD — a real RNE case!), mul (2.5×2.0=5.0; 1.0/3.0-style inexact: 3.0×0.33333334), div (1.0/4.0=0.25; 1.0/3.0=0x3EAAAAAB — RNE), uitofp/sitofp/fptoui/fptosi (round trips: 12345→float→12345; −7→float→−7; float 100.0→100), cmp (all 4 outcomes incl. the −0==+0 case and a NaN→3 case), the fcmp predicate materialization end-to-end (a few predicates through the tree). The Rust reference in the test guards the RNE.
- [ ] **Step 2: Run to verify they fail**.
- [ ] **Step 3: Implement** per the recipes — the bulk of the milestone; verify each against the SIM reference (a wrong round/sticky/alignment fails it).
- [ ] **Step 4: Run to verify they pass** — isel + workspace.
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): soft-float runtime routines"`.

---

### Task 4: Acceptance — float.c e2e + gpasm oracle

**Files:**
- Create: `crates/driver/tests/fixtures/float.c`, `crates/driver/tests/float_e2e.rs`
- Create: `crates/asm/tests/fixtures/float.asm`, `crates/asm/tests/gpasm_float.rs`

**Interfaces:**
- Consumes: Tasks 1–3 + `pic14-sim` + gpasm. Debug in the responsible stage; keep stage tests green.

- [ ] **Step 1: Write the failing acceptance program** — float.c with hand-computable results (volatile inputs so clang keeps the ops live):

```c
volatile float out;
volatile float in;

__attribute__((noinline)) float half(float a) { return a / 2.0f; }   // fdiv call

void main(void) {
    float a = in;                        // e.g. in = 0.5f
    out = half(a);                       // 0.25 (exact)
    out = a + 0.25f;                     // 0.75 (exact)
    out = a * 3.0f;                      // 1.5 (exact)
    out = 1.0f / 3.0f;                   // 0x3EAAAAAB (RNE — hand-computable)
    out = (a < 0.75f) ? 1.0f : 0.0f;     // fcmp olt: 1.0
    out = (float)(int)(a * 100.0f) / 100.0f;  // fptosi + sitofp round trip: 0.5
    struct S { unsigned char c; float f; };   // a float struct member (byval/sret)
    ...
}
```

(Expected values **recomputed by hand from the exact emitted IR** and documented — the RNE case 0x3EAAAAAB is the load-bearing one. Keep the program small; no NaN/denormals/infinity (documented; the routines handle them deterministically but the acceptance stays in the normal range).)
- [ ] **Step 2: Write the acceptance test** — `float_e2e.rs`: run the driver, simulate, assert `out` and `halted()`. Debug in the responsible stage.
- [ ] **Step 3: Write the gpasm cross-check** — fixture the driver's `.asm`, assert our HEX == gpasm byte-for-byte + same sim `out` (M6–M14 pattern).
- [ ] **Step 4: Run the full suite** — all green (probe → fuzz → float).
- [ ] **Step 5: Commit** — `git commit -m "test(e2e): float compiles and runs correctly"`.

---

### Task 5: the float differential — the RNE verification at scale

**Files:**
- Modify: `crates/fuzz/src/lib.rs`, `crates/fuzz/tests/differential.rs`
- Test: `crates/fuzz/tests/differential.rs` (extend)

**Interfaces:**
- Consumes: Tasks 1–3 + the M14 harness. Produces: the generator's float extension (f32 inputs as random bit patterns — excluding NaN/denormals/inf for the corpus, documented; fadd/fsub/fmul/fdiv/fcmp/conversions; the checksum folds over the float BITS) + a float differential corpus (e.g. 50 float seeds, --ignored). This is the independent RNE verification (host clang's RNE vs our routines).

- [ ] **Step 1: Extend the failing tests** — a small float differential test (a few fixed seeds with hand-computable checksums).
- [ ] **Step 2: Run to verify they fail** (no float generation).
- [ ] **Step 3: Implement** — the float surface in the generator + the corpus; fix any RNE mismatches the differential reveals (the routines or the rounding — the milestone's sharpest verification).
- [ ] **Step 4: Run to verify they pass** — the float corpus clean.
- [ ] **Step 5: Commit** — `git commit -m "test(fuzz): float differential corpus"`.

---

## Self-review notes

- **Spec coverage:** M15 completes phase 7 — the roadmap is done. Remaining: only the tracked follow-ups (ISR routine duplication, GIE, i16 wrap bug, i32 const tables, dynamic memcpy, const structs, banked routines, FSR-aware alloc) and the deferred fuzz extensions.
- **Correctness risks (verify by SIMULATION against a Rust f32 reference):** (1) the RNE rounding (round bit + sticky + LSB; the tie case rounds to even; a rounding carry renormalizes) — the 0.1+0.2 and 1.0/3.0 cases are the load-bearing ones; (2) the alignment shift in add (up to 31 bits with the sticky collection); (3) the sign-magnitude comparison in cmp (−0 == +0; the sign bit ordering); (4) the mul/div mantissa math (24×24 → the top bits; the restoring division); (5) the conversion edge cases (the leading-1 search; the exponent clamp). Every one has a SIM case + the float differential.
- **The fcmp predicates** are materialized as icmp/select trees over the tri-state byte (no i1 binops — the isel rejects them); the 14 trees are documented in the legalize tests.
- **Deferred (later milestones):** double-precision (f64) — msp430's double is f32 so the C surface is covered; NaN/denormal/inf handling is deterministic-but-minimal (the acceptance and the corpus avoid them; a full IEEE edge-case suite is a follow-up).
- **Contract:** `Ty::F32`, the routine names/signatures/scratch layouts, the tri-state cmp byte (0/1/2/3), and the RNE rounding are the cross-crate contracts.
