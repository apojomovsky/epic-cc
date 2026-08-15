# Integer Spine — Milestone 6: Scalar C Surface Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** "Ordinary user C" compiles — the missing scalar operations land: `sub`, `and i8`, `or`, `xor` (8/16-bit), all ten `icmp` predicates (`eq`, `ne`, unsigned and signed orderings), and `sext`. The milestone acceptance: a program using typical embedded-C idioms — unsigned char/int arithmetic (`+ - & | ^`), comparisons (`< <= > >= != ==`), a loop — compiles through the pipeline and runs correctly, cross-checked against `gpasm`.

**Architecture:** `crates/ir` accepts all `icmp` predicates and the `sext` opcode; `crates/irparse` parses them from `.ll`; `crates/isel` lowers the missing binops (bytewise, with the borrow chain for `sub`), all comparison predicates (via the PIC14 `C`/`Z` flags from `SUBWF`/`XORWF`), and sign extension. `pic14-sim` already models all flags and instructions.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

**Spec:** [`docs/12-backend-design.md`](../12-backend-design.md) §4 phase 2; the spike's `spike/src/codegen.rs` is the verified reference for the bytewise/carry patterns.

## Global Constraints

- Build/test with `nix develop --command cargo …`; never `apt install` toolchain deps.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only; GPL never linked.
- Text boundaries: stages communicate via text; the `ir` crate defines the IR text format.
- New files must be `git add`ed before `nix develop` sees them.
- Unsupported constructs panic loudly, never silently miscompile.

## The lowering recipes (the load-bearing design)

All flag-sensitive lowerings use `SUBWF f,W` (computes `f - W`; `C` = no-borrow = `f >= W`; `Z` = `f == W`) or `XORWF f,W` (sets `Z` on equality).

**16-bit `sub`** (reg − reg, bytewise borrow chain):
```
MOVF b_lo,W; SUBWF a_lo,W; MOVWF d_lo      ; d_lo = a_lo - b_lo, C = (a_lo >= b_lo)
MOVF b_hi,W; BTFSS STATUS,0; ADDLW 1; SUBWF a_hi,W; MOVWF d_hi   ; borrow propagated
```
(When `C` is clear — borrow — `ADDLW 1` adds the borrow before subtracting; verify the exact chain against the spike's `add` carry chain mirrored.)

**`and`/`or`/`xor` i8/i16** — bytewise `ANDLW`/`ANDWF`/`IORLW`/`IORWF`/`XORLW`/`XORWF` per byte (mirror the existing `and16`).

**Comparison predicates** (a op b, a in f / b in W via `MOVF a,W; SUBWF b,W` — i.e. `b - a`, `C = (b >= a)`, `Z = (b == a)`):

| predicate | condition | materialize |
|---|---|---|
| `eq` | Z | `MOVLW 0; BTFSS STATUS,2; MOVLW 1` (Z set → skip → 0? **verify direction carefully — the existing `eq` uses the opposite; keep `eq` as-is and derive `ne` = NOT eq**) |
| `ne` | !Z | invert the `eq` materialization |
| `ult` | !C | `MOVLW 0; BTFSS STATUS,0; MOVLW 1` |
| `uge` | C | `MOVLW 0; BTFSC STATUS,0; MOVLW 1` |
| `ugt` | C && !Z | `BTFSS STATUS,0; <0>; <check Z>` — two-skip or branch |
| `ule` | !C \|\| Z | negate `ugt` |
| signed | see below | sign-aware |

**16-bit comparisons:** compare the high bytes first (the carry chain sets C/Z for the full comparison); the byte-0 compare's flags are overwritten — use the documented PIC16 multi-byte compare idiom (compare high bytes with `SUBWF` chain, then low bytes) and verify by trace. **Signed (`slt`/`sle`/`sgt`/`sge`):** use the sign-bit + overflow idiom — compare the sign bits first (`XORWF` of the high-byte sign bits into the C computation), or the standard `slt = (C==0) XOR (sign_a XOR sign_b)`; verify by exhaustive 8-bit test (all 256×256 pairs would be ideal; at minimum a representative set).

**`sext`:** copy the low bytes, then propagate the source sign bit into the high bytes (`BTFSS <src_hi>, 7; GOTO .pos; MOVLW 0xFF; GOTO .done; .pos: CLRF...` — or `MOVLW 0`/`MOVLW 0xFF` based on bit 7).

---

### Task 1: `ir` + `irparse` — all icmp predicates and `sext`

**Files:**
- Modify: `crates/ir/src/lib.rs`, `crates/irparse/src/lib.rs`
- Test: `crates/ir/tests/roundtrip.rs`, `crates/irparse/tests/parse_ll.rs` (extend)

**Interfaces:**
- Produces: `icmp` accepts predicates `eq, ne, ult, ule, ugt, uge, slt, sle, sgt, sge` (canonical text: `%d = icmp <pred> <ty> <a> <b>`); `sext` opcode (`%d = sext <ty> %v to <ty>`) as a new `Inst` variant (or a `Zext`-like variant with a sign flag — decide: a distinct `Sext` variant is cleaner). `irparse` parses them from `.ll` (`icmp slt i16 ...`, `sext i8 %v to i16`).

- [ ] **Step 1: Extend the failing tests** — ir: round-trip `%c = icmp ult i8 %a %b` and `%s = sext i8 %v to i16`; irparse: parse a `.ll` with `icmp uge i8` and `sext`.
- [ ] **Step 2: Run to verify they fail**.
- [ ] **Step 3: Implement** — widen the pred acceptance in `ir` (parse/serialize) and `irparse`; add the `Sext` variant + arms.
- [ ] **Step 4: Run to verify they pass**.
- [ ] **Step 5: Commit** — `git commit -m "feat(ir,irparse): all icmp predicates and sext"`.

---

### Task 2: `isel` — `sub` and the missing binops

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: `sub` (i8/i16, reg−reg and reg−const — the borrow chain), `and i8`, `or` (i8/i16), `xor` (i8/i16) — bytewise, mirroring the existing `add`/`and16` structure.

- [ ] **Step 1: Extend the failing test** — modules exercising each new binop (i8 and i16), asserting the emitted bytewise sequences (e.g. `SUBWF 0xNN, W`, `BTFSS STATUS, 0`, `IORWF`, `XORLW`).
- [ ] **Step 2: Run to verify they fail** (isel panics on unsupported binop).
- [ ] **Step 3: Implement** — the lowerings per the recipes; keep the i1 reject and the const-LHS normalization (commutative ops swap; `sub` is NOT commutative — const-LHS sub panics loudly or is normalized via `0 - a` if needed).
- [ ] **Step 4: Run to verify they pass** — isel tests + workspace (no regression).
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): sub, or, xor and i8 and"`.

---

### Task 3: `isel` — all comparison predicates

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: `ne`, `ult`, `ule`, `ugt`, `uge`, `slt`, `sle`, `sgt`, `sge` for i8 and i16, per the recipes. Keep `eq` as-is.

- [ ] **Step 1: Extend the failing test** — for each predicate, a module asserting the emitted skip/branch lines AND (critically) a **simulation test**: hand-assemble the emitted `.asm` for a few operand pairs and run it in `pic14_sim` asserting the i1 result — this validates the flag logic end-to-end. The spike's probe pattern (in/out globals + `pic14_sim::parse_hex`) is the model. At minimum: `ult` (5 < 9 → 1; 9 < 5 → 0), `ugt`, `slt` (−1 < 1 → 1; 1 < −1 → 0), 16-bit variants.
- [ ] **Step 2: Run to verify they fail**.
- [ ] **Step 3: Implement** — the flag recipes; verify each against the simulation tests (a wrong flag direction fails the sim).
- [ ] **Step 4: Run to verify they pass** — isel + workspace.
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): all comparison predicates"`.

---

### Task 4: `isel` — `sext`

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: `sext` — copy the low bytes, then fill the high bytes with the source's sign bit (0x00 if the MSB is clear, 0xFF if set).

- [ ] **Step 1: Extend the failing test** — `%s = sext i8 %v to i16`: assert the copy + the sign-fill branch (e.g. `BTFSS <src_hi>, 7` + `MOVLW 0xFF`/`CLRF`). A simulation test for −1 and +1 is ideal.
- [ ] **Step 2: Run to verify it fails**.
- [ ] **Step 3: Implement** — the recipe.
- [ ] **Step 4: Run to verify it passes**.
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): sign extension"`.

---

### Task 5: Acceptance — an ordinary embedded-C program

**Files:**
- Create: `crates/driver/tests/fixtures/scalar.c`, `crates/driver/tests/scalar_e2e.rs`
- Create: `crates/asm/tests/fixtures/scalar.asm`, `crates/asm/tests/gpasm_scalar.rs`

**Interfaces:**
- Consumes: the full pipeline (Tasks 1–4) and `pic14_sim`.

- [ ] **Step 1: Write the failing acceptance program** — `scalar.c`: a loop over a small computation using the newly supported ops, with a hand-computable result:

```c
volatile unsigned char in;
volatile unsigned char out;
void main(void) {
    unsigned char n = in & 0x07;
    unsigned char s = 0;
    unsigned char i;
    for (i = 0; i < n; i++) {
        if ((i & 1) == 0) s = (unsigned char)(s + (i * 3));
        else              s = (unsigned char)(s - (i * 2) ^ (i << 1));
    }
    out = s;
}
```

(Adjust if `-O1` folds or emits constructs isel doesn't support — the goal is a program that exercises `sub`, `and i8`, `or`/`xor`, and several comparison predicates, with a hand-computed `out` for `in = 7`. Note `i * 3` / `i << 1` may become `mul`/`shl` — if so, replace with `i + i + i` / `i + i` to stay in the supported surface. Document the exact expected value in the test.)
- [ ] **Step 2: Write the acceptance test** — `scalar_e2e.rs` runs the driver, simulates with `in = 7`, asserts the hand-computed `out` and `halted()`. Debug in the responsible stage (isel likely); keep stage tests green.
- [ ] **Step 3: Write the gpasm cross-check** — capture the driver's `.asm`, fixture it, assert our HEX == gpasm byte-for-byte + sim run gives the same `out` (mirror the M5 pattern).
- [ ] **Step 4: Run the full suite** — all green (probe, overlay, banked, ptr_probe, array, scalar).
- [ ] **Step 5: Commit** — `git commit -m "test(e2e): ordinary scalar C compiles and runs correctly"`.

---

## Self-review notes

- **Spec coverage:** milestone 6 completes the integer-spine scalar surface — every binop, every comparison predicate, and sign extension — so ordinary embedded C compiles. Structs (`sret`/`byval`) are the next milestone.
- **Correctness notes for the implementer:** the comparison lowerings are the risk — every recipe must be verified by **simulation** (the Task-3 tests run the emitted asm through `pic14_sim`), not just by asserting emitted strings, because a wrong flag direction passes a string test and fails the sim. The signed predicates and the 16-bit orderings deserve the most scrutiny (exhaustive 8-bit testing where feasible). The `eq` materialization must stay byte-identical (existing tests depend on it).
- **Deferred (later milestones):** structs/sret/byval; constant folding (both-const binops still panic); `mul`/`div`/`shl`/`shr` (clang emits these for `*`, `<<` — the acceptance avoids them; a runtime-library milestone is required for real use).
- **Type consistency:** the icmp predicate strings (`eq/ne/ult/ule/ugt/uge/slt/sle/sgt/sge`) and the `Sext` variant are the contracts across ir/irparse/isel.
