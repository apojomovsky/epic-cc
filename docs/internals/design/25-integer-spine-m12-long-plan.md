# Integer Spine — Milestone 12: 32-bit `long` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `long`/`unsigned long` (i32) compiles — phase 5's 32-bit half. `Ty::I32` flows through the whole pipeline: arithmetic, all ten comparisons, casts, shifts, mul/div/mod via new runtime routines, calls with i32 params/returns, structs with i32 members. Acceptance: a `long` program with hand-computable output runs correctly and our HEX matches gpasm byte-for-byte.

**Architecture:** `crates/ir` + `crates/irparse` gain `Ty::I32` (bytes 4, msp430 align 2) and the struct layout for i32 fields; `crates/alloc` is automatic (`ty.bytes()`); `crates/isel` extends the byte-generic recipes (`emit_cmp_eq`/`emit_cmp_c`/`emit_commutative` already loop over `ty.bytes()`), adds `add32`/`sub32` carry chains, widens zext/sext/trunc and the retval area (0x71–0x74), and generalizes the icmp ordering's 16-bit special case; `crates/legalize` maps the i32 ops to new routines (`__mul_u32`, `__udiv/__urem_u32`, `__sdiv/__srem_i32`, `__shl/__lshr_u32`, `__ashr_i32`); `crates/isel` emits those routine bodies (AN526 32-iteration shift-add with 4-byte wrapping accumulators; restoring divmod with 4-byte chains; sign wrappers; shift loops with count masked to 31). All verification via `pic14-sim` + gpasm oracle.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

## Global Constraints

- Build/test with `make exec CMD="cargo ..." …`; never bare `cargo`.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only.
- Unsupported constructs panic loudly, never silently miscompile.

## The lowering recipes (load-bearing design)

### The clang surface (probed, /tmp/m7probe/l1.ll)

- `add/sub/mul/udiv/urem/sdiv/srem/shl/lshr/ashr/and/or/xor i32`, `icmp <pred> i32`, `zext/sext/trunc` between i8/i16/i32, `zext i1 to i32`, `load/store i32`, calls with i32 params/returns, `freeze i32`. (clang folds const-only cases at -O1 — the acceptance uses volatile inputs to keep the ops live, like M8's muldiv.)
- `a % 7` compiles to `udiv` + `mul` + `sub` — **i32 mul is required for `%`**.

### Ty::I32 (ir/irparse/alloc)

- `Ty::I32`, `bytes() == 4`. `ty_of("i32")` parses. The canonical text uses `i32` verbatim (all Inst serializers are type-driven — automatic).
- Struct layout (irparse): an i32 field is size 4, align 2 (msp430 `i32:16`), so `{ i8, i32 }` → i8@0, i32@2, size 6 (round_up(6, 2) = 6). The GEP byte-offset folding (clang) handles member access — only the SIZE feeds globals/allocas/byval.
- alloc: `ty.bytes()` everywhere — automatic.

### isel — the 4-byte extensions

- **Retval area**: i32 returns need 4 bytes — the fixed retval region widens to `0x71–0x74` (scratch stays 0x70; common RAM 0x70–0x7F has room). `emit_call`'s retval copy loops `t.bytes()` (already generic — verify the region math: retval_lo + 3 ≤ 0x7F).
- **add/sub**: `emit_add32`/`emit_sub32` — the carry/borrow chain across 4 bytes, mirroring the i16 chains (`BTFSS STATUS,0; ADDLW 1` borrow propagation; the C-chain for add).
- **and/or/xor**: `emit_commutative` is already `ty.bytes()`-looped ✓ (verify the const-LHS swap and the opcode selection handle i32).
- **icmp**: `emit_cmp_eq` (scratch-accumulate XOR) and `emit_cmp_c` (SUBWF borrow chain) are already `ty.bytes()`-looped ✓; the ordering materialization's 16-bit special case (`if need_z && ic.ty.bytes() == 2` at ~line 1394) must become byte-generic (the Z-accumulation for `ugt`/`ule` across 4 bytes); the signed sign-complement applies to the HIGH byte (byte 3).
- **zext/sext/trunc**: zext i1/i8/i16 → i32 (copy + clear the high bytes); sext (copy lo bytes, sign-fill the high bytes from the SOURCE's sign byte — e.g. i16→i32 fills bytes 2–3 from bit 7 of byte 1; i8→i32 fills 1–3 from bit 7 of byte 0; the current `from.bytes()==1 && to.bytes()==2` assert widens); trunc i32 → i8/i16 (copy the low bytes — generic).
- **shifts**: inline const counts — 4-byte `rlf`/`rrf` chains (the k ≥ width panic uses `width = bytes*8` — generic); variable counts → the `__shl_u32`/`__lshr_u32`/`__ashr_i32` routines (count masked to 31).
- **load/store/select/freeze**: byte-looped ✓ (the const-global load path still asserts `bytes == 1` — i32 const tables/globals panic loudly, deferred; the acceptance avoids them).
- **The routines**: `__mul_u32` (32-iteration AN526: 4-byte tmp shifting left with wraparound + 4-byte r accumulation, the incfsz carry idiom across 4 bytes; result = low 32 bits — i32 `mul` wraps); `__udiv_u32`/`__urem_u32` (restoring division, 32 iterations, 4-byte num/rem/den with the borrow chain); `__sdiv_i32`/`__srem_i32` (abs both, sign-XOR quotient / dividend-sign remainder — mirror the i16 wrappers); `__shl_u32`/`__lshr_u32`/`__ashr_i32` (masked count, 4-byte shift loops, ashr sign-fill). Scratch layouts per routine (mul32: tmp 4 + r 4 + bk 2 + cnt 1 = 11; divmod32: rem 4 + den 4 + cnt 1 + restore scratch = 10; signed: + 2; shifts: 1–2) — documented as the cross-task contract. Bank-0 ≤ 0x7F asserts (the M8 rule).

---

### Task 1: `ir` + `irparse` + `alloc` — `Ty::I32`

**Files:**
- Modify: `crates/ir/src/lib.rs`, `crates/irparse/src/lib.rs`, `crates/alloc/src/lib.rs`
- Test: `crates/ir/tests/roundtrip.rs`, `crates/irparse/tests/parse_ll.rs`, `crates/alloc/tests/alloc.rs` (extend)

**Interfaces:**
- Produces: `Ty::I32` (bytes 4); `i32` parses in every type position; struct layout: i32 fields size 4 align 2 (`{i8, i32}` → 6, `{i32, i8}` → 6); def_width automatic; i32 params/returns/allocas sized 4.

- [ ] **Step 1: Extend the failing tests** — roundtrip `add i32`, `icmp ult i32`, `zext i8 to i32`, `trunc i32 to i8`; parse_ll a `.ll` with the l1.ll shapes; struct layout: `%struct.S = type { i8, i32 }` sizes a global to 6; alloc places an i32 param/def at 4 bytes.
- [ ] **Step 2: Run to verify they fail** (no I32 variant).
- [ ] **Step 3: Implement** — the Ty variant + ty_of + the struct-layout arm. (isel still panics on i32 binops — Task 2; add minimal loud arms so the workspace builds.)
- [ ] **Step 4: Run to verify they pass** + workspace builds.
- [ ] **Step 5: Commit** — `git commit -m "feat(ir,irparse,alloc): 32-bit type"`.

---

### Task 2: `isel` — i32 arithmetic, compares, casts, shifts

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: `emit_add32`/`emit_sub32` carry chains; the icmp ordering's Z special case generalized to `ty.bytes()`; zext/sext/trunc widened (sext sign-fills from the source's sign byte); the retval area widened to 0x71–0x74 (verify the region math); inline const shifts at 4 bytes (k ≥ 32 panics); the const-global load assert stays (`bytes == 1` — i32 const tables deferred, loud).

- [ ] **Step 1: Extend the failing tests** — emitted-asm asserts: `add i32` (the 4-byte carry chain), `sub i32` (borrow chain), `icmp ult i32` + `ugt i32` (the Z accumulation across 4 bytes), `sext i16 to i32` (sign-fill bytes 2–3), `zext i8 to i32`, `trunc i32 to i8`, `shl i32` ×3 (4 rlf chains), `ashr i32` (sign-fill from byte 3); the i32 call retval copies 4 bytes from 0x71. **SIM (load-bearing):** a module computing with i32 add/sub/icmp/shift/zext/sext/trunc against fixed inputs — assembled + run, results asserted (e.g. 0x12345678 + 0x00000005 = 0x1234567D; 0x80000000 >> 2 = 0xE0000000; (0x12345678 < 0x20000000) = 1; sext i16 0x8000 → 0xFFFF8000).
- [ ] **Step 2: Run to verify they fail** (isel panics on i32).
- [ ] **Step 3: Implement** per the recipes.
- [ ] **Step 4: Run to verify they pass** — isel + workspace (the i8/i16 paths byte-identical — the byte-generic loops unchanged).
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): 32-bit arithmetic, compares, casts and shifts"`.

---

### Task 3: `legalize` + `isel` — the i32 runtime routines

**Files:**
- Modify: `crates/legalize/src/lib.rs`, `crates/isel/src/lib.rs`
- Test: `crates/legalize/tests/legalize.rs`, `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: legalize maps i32 `mul/udiv/urem/sdiv/srem` + reg-count `shl/lshr/ashr` to `__mul_u32`/`__udiv_u32`/`__urem_u32`/`__sdiv_i32`/`__srem_i32`/`__shl_u32`/`__lshr_u32`/`__ashr_i32` (i32 params, i32 ret) and injects the routine Funcs with scratch allocas (layouts documented as the contract); isel emits the routine bodies (panic-first on the new names until the recipes land, then the recipes).

- [ ] **Step 1: Extend the failing tests** — legalize: `mul i32` → `call i32 @__mul_u32` + the injected def; isel: panic-first on the 8 new routine names; **SIM (load-bearing)**: fixed-input routines — mul_u32 (0x00010001 × 0x00010001 = 0x00020001), udiv_u32 (0x12345678 / 0x100 = 0x12345, rem 0x78), urem_u32, sdiv_i32 (−19 / 3 = −6; INT_MIN / −1... 0x80000000 / −1 = 0x80000000 — wrapping, poison-adjacent but deterministic — document), srem_i32 (−19 % 3 = −1), the shifts (x << 3; x >> 17 with the 31-mask; ashr of 0x80000000 >> 4 = 0xF8000000), each assembled + run with asserted results.
- [ ] **Step 2: Run to verify they fail**.
- [ ] **Step 3: Implement** — the legalize mappings + the recipes (the M8 16-bit patterns extended to 4 bytes; the incfsz carry idiom; the divmod borrow/restore; the sign wrappers mirroring the i16 XOR logic).
- [ ] **Step 4: Run to verify they pass** — legalize + isel + workspace.
- [ ] **Step 5: Commit** — `git commit -m "feat(legalize,isel): 32-bit mul, div, rem and shift routines"`.

---

### Task 4: Acceptance — long.c e2e + gpasm oracle

**Files:**
- Create: `crates/driver/tests/fixtures/long.c`, `crates/driver/tests/long_e2e.rs`
- Create: `crates/asm/tests/fixtures/long.asm`, `crates/asm/tests/gpasm_long.rs`

**Interfaces:**
- Consumes: Tasks 1–3 + `pic14_sim` + gpasm. Debug in the responsible stage; keep stage tests green.

- [ ] **Step 1: Write the failing acceptance program** — long.c with a hand-computable `out`, all inputs volatile (clang folds const-only cases):

```c
volatile unsigned long out;
volatile unsigned long in;

__attribute__((noinline)) unsigned long addm(unsigned long a, unsigned long b) { return a + b; }

void main(void) {
    unsigned long a = in;                      // e.g. in = 0x12345678
    out = addm(a, 5);                          // i32 call param/ret: 0x1234567D
    out = a * 7;                               // mul i32: 0x12345678*7 = 0x7F6A9E48 (mod 2^32)
    out = a / 0x100;                           // udiv i32: 0x123456
    out = (out * 7) + (a % 0x100);             // urem via the % idiom: (0x123456*7 + 0x78) = 0x7F6A9E... recompute
    out = (out << 3) | (out >> 1);             // shl/lshr i32
    out = (a < 0x20000000) ? 1 : 0;            // icmp ult i32
    long s = (long)a;                          // i32 sign-agnostic copy
    out = (unsigned long)(s >> 4);             // ashr i32 (sign-fill)
    out = (unsigned long)(int)(a / 7);         // hmm — keep it simple: out = a % 7; sdiv via a volatile signed input
    ...
}
```

(Expected value **recomputed by hand from the exact emitted IR** and documented in the test — the exact `out` depends on which pieces clang keeps live; keep the coverage: i32 add (call), mul, udiv, urem, shl, lshr, icmp, ashr, zext/sext/trunc, a signed div/rem on a volatile `long`, a struct with a `long` member (e.g. `struct P { unsigned char a; unsigned long b; };` — byval/sret with the 6-byte layout). No i32 const tables (deferred — the acceptance stays clear of them).)
- [ ] **Step 2: Write the acceptance test** — `long_e2e.rs`: run the driver, simulate, assert `out` and `halted()`. Debug in the responsible stage.
- [ ] **Step 3: Write the gpasm cross-check** — fixture the driver's `.asm`, assert our HEX == gpasm byte-for-byte + same sim `out` (M8–M11 pattern).
- [ ] **Step 4: Run the full suite** — all green (probe → multi_page → long).
- [ ] **Step 5: Commit** — `git commit -m "test(e2e): 32-bit long compiles and runs correctly"`.

---

## Self-review notes

- **Spec coverage:** M12 completes phase 5's 32-bit half (soft mul/div/mod already landed in M8 for 8/16-bit; the i32 routines extend them). Remaining roadmap: interrupts+SFR (phase 4), random testing (phase 6), soft-float (phase 7).
- **Correctness risks (verify by SIMULATION):** (1) the 4-byte carry/borrow chains (add/sub/icmp ordering) — a wrong chain direction flips every 32-bit result; (2) the i32 mul's wrapping accumulator (the shifted-out high bits must be DISCARDED — the 4-byte tmp shifts wrap); (3) the divmod borrow/restore across 4 bytes; (4) sext sign-fill from the source's sign byte (i16→i32 fills bytes 2–3 from byte 1's bit 7 — NOT byte 3); (5) the retval 4-byte region (0x71–0x74 must not collide with scratch 0x70 or the routines' frames); (6) the icmp ordering Z-special-case generalization. Every one has a sim test.
- **Deferred (later milestones):** i32 const tables/globals (the const-read path stays i8 — loud panic); constant folding (const-const i32 ops still panic); i64; the phase-4/6/7 chunks.
- **Contract:** `Ty::I32` (bytes 4, align 2), the routine names/signatures + scratch layouts, the retval region 0x71–0x74, and the zext/sext/trunc widening are the cross-crate contracts.
