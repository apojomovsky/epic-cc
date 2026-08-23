# Integer Spine — Milestone 8: mul/div/shift Runtime Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the last big scalar surface lands — `mul`, `udiv`/`urem`, `sdiv`/`srem`, `shl`/`lshr`/`ashr` (i8 + i16), so ordinary embedded C with `* / % << >>` compiles and runs. Multiplication/division are **runtime-library routines** (PIC16F877A has no hardware multiply — shift-add/restoring-division), adapted from the **machine-verified PIC16 asm in `/home/alexis/projects/epicurus/epic-math/src/pic16/`** (epic_math_mul.c = AN526 shift-add; epic_math_div.c = restoring shift-subtract). Acceptance: a `* / % << >>` program runs correctly in the simulator and our HEX matches gpasm byte-for-byte.

**Architecture:** `crates/ir` + `crates/irparse` accept the new binops and `freeze`; `crates/legalize` (its doc already names "runtime calls for mul/div") rewrites the new binops into calls to injected runtime functions and appends the routine definitions (with scratch allocas) to the module; `crates/isel` emits the routine bodies from hand-written recipes (adapted from epicurus) and inlines constant-count shifts; alloc needs no change (the routines are ordinary functions). All verification via `pic14-sim` + gpasm oracle.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

**Spec:** completes the integer-spine scalar surface (docs/12 §4 phase 2). Reference algorithms: `/home/alexis/projects/epicurus/epic-math/src/pic16/epic_math_mul.c` and `epic_math_div.c` (user's repo; AN526/AN544 family; machine-verified against MPLAB SIM in that repo's test gates; its `tests/test_mul.c`/`test_div.c` hold the C reference algorithms).

## Global Constraints

- Build/test with `make exec CMD="cargo ..." …`; never bare `cargo`.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only.
- Text boundaries: `crates/ir` defines the canonical IR text format; `alloc` map and `.asm` are text artifacts.
- Unsupported constructs panic loudly, never silently miscompile.

## The lowering recipes (load-bearing design)

### The clang surface (probed, /tmp/m7probe/m1-m4.ll)

- `mul i16/i8` (wrapping; signedness irrelevant). Note: `a % 7` compiles to `udiv` + `mul` + `sub` — **mul is required to implement `%`**.
- `udiv`/`urem` i8/i16 (m3: `udiv i16 %2, 7`).
- `sdiv`/`srem` i8/i16 with constant signed divisors (m4: `sdiv i16 %1, -3`, `srem i16 %1, 3`).
- `shl`/`lshr` with constant counts (m3: `shl i16 %6, 3`) **and variable counts** (m3: `shl i16 %1, %11` — the count arrives UNMASKED, full i16).
- `ashr` (m4: `ashr i16 %1, 2`) — arithmetic shift (sign-fill).
- **`freeze i16 %1`** appears before `udiv` (m3) — must parse and lower as a plain copy.

### Runtime routines (the ABI)

The routines are **ordinary module functions** injected by legalize: `Func { name, ret, params, blocks }` with empty blocks plus injected `Inst::Alloca` scratch defs (so alloc places the scratch in the routine's frame; isel's recipe emitter resolves `{func}::__scr` from the map). The caller side is the existing call ABI — legalize rewrites `Inst::Bin` into `Inst::Call` with the same dst/ty, and `emit_call` copies args into the routine's param slots, CALLs, copies the retval. Recipes emit their asm with **plain addresses** (banking pass handles BANKSEL); **assert every routine slot address ≤ 0xFF** (bank-0 only, same documented limitation as FSR bases — the routines' loops contain skip-sensitive sequences where an inserted BANKSEL would change skip targets; keep them bank-0, loudly).

Routine set (16), signatures:

| IR op (i8) | routine | IR op (i16) | routine |
|---|---|---|---|
| `mul` | `__mul_u8(i8,i8) -> i8` (lo of 16-bit product) | `mul` | `__mul_u16(i16,i16) -> i16` (lo 16 of 32-bit) |
| `udiv` / `urem` | `__udiv_u8` / `__urem_u8` | `udiv` / `urem` | `__udiv_u16` / `__urem_u16` |
| `sdiv` / `srem` | `__sdiv_i8` / `__srem_i8` | `sdiv` / `srem` | `__sdiv_i16` / `__srem_i16` |
| `shl`/`lshr`/`ashr` (variable cnt) | `__shl_u8` / `__lshr_u8` / `__ashr_i8` | (variable) | `__shl_u16` / `__lshr_u16` / `__ashr_i16` |

Semantics:
- **mul**: AN526 shift-add, adapted from epic_math_mul.c. 8x8: tmp = a shifted left per set bit of b, r += tmp; carry idiom `movf t_hi,w; btfsc STATUS,0; incfsz t_hi,w; addwf r_hi,f`. Store the product's low byte(s) to retval (i8/i16 result width).
- **divmod**: restoring shift-subtract, adapted from epic_math_div.c: `num <<= 1 (rlf chain); rem = (rem<<1)|carry; if (rem >= den) { rem -= den; num |= 1 } else restore (add den back)`; borrow idiom `movf den_hi,w; btfss STATUS,0; incfsz den_hi,w; subwf rem_hi,f`. `__udiv_*` stores the quotient, `__urem_*` the remainder (each runs the full loop; both computed). 8-bit: 8 iterations; 16-bit: 16 iterations.
- **sdiv/srem**: sign wrapper — abs both operands (unsigned abs, so INT_MIN abs is safe), run the unsigned divmod, negate the quotient if the signs differed (sdiv) / the remainder if the dividend was negative (srem, sign follows the dividend).
- **Div-by-zero** is C UB / LLVM poison: the loop runs (den=0 ⇒ quotient 0xFFFF, remainder 0) — any value is legal; document in the routine comments, no guard.
- **shifts (variable count)**: count is masked to `width-1` in the routine (UB-range count ⇒ any value is legal; masking keeps the loop bounded, ≤ 16 iterations, and gives the correct result in the defined range). Loop: `shl`: `bcf STATUS,0; rlf val_lo,f; rlf val_hi,f` per iteration × cnt; `lshr`: `rrf` chain; `ashr`: sign-fill — copy the sign bit into C (`btfsc val_hi,7; bsf STATUS,0; btfss val_hi,7; bcf STATUS,0` — or set C from bit 7), then `rrf` chain.
- **shifts (constant count)**: INLINE in isel (no call): `RLF`/`RRF` × k; `ashr` sets C from the sign bit before each `rrf`; k == 0 → plain copy; **k ≥ width panics loudly** (LLVM poison; no legitimate program relies on it — loud beats deterministic-garbage). i8: 1 byte; i16: 2 bytes.

### freeze

`%d = freeze <ty> %v` → `Inst::Freeze { dst, ty, val }`, canonical `%d = freeze <ty> <val>`. Semantics for our whole-program pipeline: a copy (`emit_move_val_to_slot`). Poison is never generated by our frontend path (volatile loads aren't poison); clang emits freeze defensively before `udiv`.

---

### Task 1: `ir` + `irparse` — the new binops and `freeze`

**Files:**
- Modify: `crates/ir/src/lib.rs`, `crates/irparse/src/lib.rs`
- Test: `crates/ir/tests/roundtrip.rs`, `crates/irparse/tests/parse_ll.rs` (extend)

**Interfaces:**
- Produces: `BinOp` gains `Mul, UDiv, URem, SDiv, SRem, Shl, LShr, AShr` (serialize as `mul`/`udiv`/`urem`/`sdiv`/`srem`/`shl`/`lshr`/`ashr`); `Inst::Freeze`; irparse parses all nine opcodes + `freeze` (with the usual attr stripping; the `.ll` forms: `%3 = mul i16 %2, 7`, `%3 = udiv i16 %2, 7`, `%4 = ashr i16 %1, 2`, `%2 = freeze i16 %1`).

- [ ] **Step 1: Extend the failing tests** — roundtrip each new binop + freeze; parse_ll a `.ll` with mul/udiv/sdiv/shl/lshr/ashr/freeze lines (from the m3/m4 probe shapes).
- [ ] **Step 2: Run to verify they fail**.
- [ ] **Step 3: Implement** — ir enum/Inst/serialize; irparse arms. Add the minimal isel `Inst::Freeze` lowering (byte copy — it's self-contained) so the workspace builds; the new binops keep a loud panic in isel until Task 3.
- [ ] **Step 4: Run to verify they pass** + workspace build.
- [ ] **Step 5: Commit** — `git commit -m "feat(ir,irparse): mul, div, rem, shift binops and freeze"`.

---

### Task 2: `legalize` — runtime-call lowering + routine injection

**Files:**
- Modify: `crates/legalize/src/lib.rs`, `crates/driver/src/main.rs` (nothing — legalize is already called)
- Test: `crates/legalize/tests/legalize.rs` (extend; the crate currently has a pass-through test)

**Interfaces:**
- Produces: `legalize` rewrites `Inst::Bin` — `mul/udiv/urem/sdiv/srem` (i8/i16) → `Inst::Call` to the matching routine (dst/ty preserved, args copied); `shl/lshr/ashr` with a **const count stay as Bin** (isel inlines); with a **reg count → Call** to the shift routine. `freeze` stays (isel copies). After rewriting, inject the used routine `Func`s (name/ret/params per the table + one empty block containing the scratch allocas: `__mul_u8`: `%__scr = alloca 6` (bk,cnt,r_lo,r_hi,t_lo,t_hi); `__mul_u16`: 14; `__udiv_u8`/`__urem_u8`: 4 (rem_lo,rem_hi,cnt,restore scratch); `__udiv_u16`/`__urem_u16`: 7; signed wrappers: 5; shifts: 3 (cnt, plus the value is in params)). Exact scratch layouts are defined in Task 3's recipes — coordinate via the shared contract (document the layout table in this task's report and in the code). Inject only the routines actually used (cleaner text artifacts).

- [ ] **Step 1: Extend the failing tests** — legalize a module with `mul i16` + variable `shl i16` + const `shl i16`: assert the rewritten text has `call i16 @__mul_u16` + the injected `fn __mul_u16` def (params + alloca), the variable shift became `call i16 @__shl_u16`, the const shift stayed `shl i16`. Also run it through `alloc::allocate` and assert the routine's param/scratch slots exist in the layout.
- [ ] **Step 2: Run to verify they fail** (pass-through legalize leaves mul as Bin).
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run to verify they pass** — legalize tests + workspace (driver e2e still green; isel still panics on the new binop calls — Task 3).
- [ ] **Step 5: Commit** — `git commit -m "feat(legalize): lower mul, div, rem and shifts to runtime calls"`.

---

### Task 3: `isel` — mul/div/rem routine bodies

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: when `select` reaches a function named `__mul_u8`/`__mul_u16`/`__udiv_u8`/`__urem_u8`/`__udiv_u16`/`__urem_u16`/`__sdiv_i8`/`__srem_i8`/`__sdiv_i16`/`__srem_i16`, emit the recipe body instead of the (empty) block insts. Recipes adapted from `/home/alexis/projects/epicurus/epic-math/src/pic16/epic_math_mul.c` / `epic_math_div.c`, with our conventions: args in `{func}::{param}` slots, result in the retval slot (`retval_lo`, 2 bytes), scratch in `{func}::__scr` (+ offsets per the layout table from Task 2). Assert every slot address ≤ 0xFF (bank-0, loud). The call side already works via `emit_call` — no caller changes.

- [ ] **Step 1: Extend the failing tests** — for each routine: a module with the matching call (from legalize's rewrite) → assert the emitted asm contains the routine's label + the expected instruction pattern (e.g. `__mul_u8` has the 8 `btfss`/`addwf` steps). PLUS **simulation tests (load-bearing)**: hand-assemble modules that call each routine with fixed inputs and assert the result: mul_u8 (e.g. 35×7=245, 200×200=0x9C40→lo 0x40 for i8... use i16 mul for the wide case: 300×7=2100), udiv_u16 (301/7=43), urem_u16 (301%7=0), udiv_u8 (200/3=66), urem_u8 (200%3=2), sdiv_i16 (−19/−3=6), srem_i16 (−19%3=−1→0xFFFF), sdiv_i8 (−128/−2=64), srem_i8 (−5%3=−2→0xFE). At least one div-by-zero case (documented poison — assert whatever the routine produces, pinned).
- [ ] **Step 2: Run to verify they fail** (isel panics on the unknown function names / new binops).
- [ ] **Step 3: Implement** — the recipes; verify each against the sim tests (a wrong borrow/carry idiom fails the sim).
- [ ] **Step 4: Run to verify they pass** — isel + workspace (existing tests green — the routine Funcs are just functions).
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): mul, div and rem runtime routines"`.

---

### Task 4: `isel` — shifts (inline const, variable via routines)

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: `Bin(Shl/LShr/AShr, i8|i16)` with a **const count** → inline RLF/RRF sequences (`bcf STATUS,0; rlf val_lo,f; rlf val_hi,f` per step; ashr sets C from the sign bit first — `btfsc val_hi,7; bsf STATUS,0; btfss val_hi,7; bcf STATUS,0` — then `rrf` chain); k==0 → plain copy; k ≥ width → panic loudly. **Reg count** → the calls legalize already emitted (`__shl_u8`/`__lshr_u8`/`__ashr_i8`/16) — emit those routine bodies here (mask count & (width−1), bounded loop; ashr sign-fill). Bank-0 asserts as in Task 3.

- [ ] **Step 1: Extend the failing tests** — asm asserts: `shl i16 %a, 3` → 3 × (bcf/rlf/rlf); `ashr i8 %a, 2` → C-from-sign + 2 × rrf; `shl i16 %a, 0` → copy only; `shl i16 %a, 16` → should_panic. SIM: `(x << 3) >> 1` with x=5 → 20; `ashr` of a negative i16 (0x8005 >> 2 = 0xE001); variable shifts `x << n` and `x >> n` with n from a volatile input (incl. n ≥ 16 → masked result, pinned).
- [ ] **Step 2: Run to verify they fail**.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run to verify they pass** — isel + workspace.
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): shift lowerings (inline const, runtime variable)"`.

---

### Task 5: Acceptance — muldiv.c e2e + gpasm oracle

**Files:**
- Create: `crates/driver/tests/fixtures/muldiv.c`, `crates/driver/tests/muldiv_e2e.rs`
- Create: `crates/asm/tests/fixtures/muldiv.asm`, `crates/asm/tests/gpasm_muldiv.rs`

**Interfaces:**
- Consumes: Tasks 1–4 + `pic14_sim` + gpasm. Debug in the responsible stage; keep stage tests green.

- [ ] **Step 1: Write the failing acceptance program** — muldiv.c covering `* / % << >>` on both i8 and i16, signed and unsigned, with a hand-computable `out`:

```c
volatile unsigned int out;
volatile unsigned int in;

void main(void) {
    unsigned int a = in;                 // e.g. in = 301
    out = a / 7;                         // 43
    out = (out * 3) + (a % 7);           // 43*3 + 0 = 129
    out = out << 2;                      // 516
    out = (out >> 3) | (a >> 4);         // 64 | 18 = 82
    int b = -19;
    out = (unsigned int)(b / -3);        // sdiv: 6
    out = (unsigned int)(b % 3) + out;   // srem: -1 (0xFFFF) + 6 = 5
    unsigned char c = (unsigned char)a;  // 45
    out = (unsigned int)((c * 7) / 3);   // i8 mul + udiv: 315/3 = 105
    out = out + ((unsigned int)c << (unsigned char)(a & 3));  // variable shl: 105 << 1 = 210
}
```

(Expected `out == 210` for `in == 301`. **Verify by hand during the task** — clang -O1 may fold/strength-reduce any piece (e.g. `* 3` → `(a<<1)+a`); if a piece disappears, adjust the C to keep the same semantic coverage and recompute; document the exact emitted IR + final value in the test. Keep the program small so routine/FSR targets stay ≤ 0xFF — the bank-0 asserts are loud if not.)
- [ ] **Step 2: Write the acceptance test** — `muldiv_e2e.rs`: run the driver, simulate with `in = 301`, assert `out` and `halted()`. Debug in the responsible stage (legalize rewrite → routine recipes → shift inline).
- [ ] **Step 3: Write the gpasm cross-check** — fixture the driver's `.asm`, assert our HEX == gpasm byte-for-byte + same sim `out` (M6/M7 pattern).
- [ ] **Step 4: Run the full suite** — all green (probe, overlay, banked, ptr_probe, array, scalar, structs, muldiv).
- [ ] **Step 5: Commit** — `git commit -m "test(e2e): mul, div, mod and shifts compile and run correctly"`.

---

## Self-review notes

- **Spec coverage:** M8 completes the scalar surface — every LLVM integer op clang can emit for i8/i16 now compiles. The remaining big surface: multi-bank FSR+IRP, const-table PCLATH page-crossing, `Global.addr` u16, dynamic-length memcpy, const structs — all deferred.
- **Correctness risks (verify by SIMULATION):** (1) the mul carry idiom (`incfsz` trick) across the 16-bit product; (2) the divmod borrow/restore idiom (a wrong `btfss STATUS,0` direction flips every result); (3) the signed wrappers (INT_MIN abs, remainder sign follows the dividend); (4) variable-shift masking; (5) ashr sign-fill. Every routine must have a sim test with hand-computed inputs; the epicurus code is the machine-verified reference — adapt faithfully, don't reinvent.
- **The banking-pass hazard:** the recipes' loops contain skip-sensitive sequences (`btfss` + branch); an inserted BANKSEL between a test and its target would break the skip. The bank-0 assert (≤ 0xFF) on every routine slot prevents this — loud, documented (multi-bank runtime routines are a follow-up).
- **Contract:** the routine names/signatures and the scratch layout table are the cross-task contracts (Task 2 injects, Task 3/4 emit). `freeze` = copy. Const-count shifts ≥ width panic loudly. Div-by-zero and ≥-width variable shifts are poison (documented, deterministic).
