# Integer Spine — Milestone 13: Interrupts + SFR/Device Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** phase 4's first half — interrupts and SFR access. C code can (1) read/write SFRs at absolute addresses (`inttoptr` — e.g. `*(volatile unsigned char *)0x06`), and (2) declare an interrupt handler (`__attribute__((interrupt(N)))`) that the compiler places at the PIC16F877A's interrupt vector (word 0x0004) with a proper save/restore prologue/epilogue and `RETFIE`. The approved ruling — **duplicate interrupt/main shared functions** — is implemented: a function reachable from both the main and ISR contexts gets an `_isr` copy, and the ISR context's frame region is **disjoint from main's** (preemption-safe overlay). Acceptance: an interrupt-driven program (SFR writes, a shared helper, the ISR fired mid-run by the simulator) runs correctly and our HEX matches gpasm byte-for-byte.

**Architecture:** `crates/ir` + `crates/irparse` mark the ISR (`msp430_intrcc` — clang's interrupt calling convention — in the return position) and parse `inttoptr (i16 k to ptr)` constant pointers; `crates/isel` emits the vector entry at word 4 (`.org 4`), the ISR save prologue / restore epilogue / `RETFIE` (save area = fixed common RAM 0x75–0x78: W, STATUS, PCLATH, FSR), and direct SFR load/store through literal pointers (bank-independent, no BANKSEL); `crates/legalize` duplicates the shared functions (`f` → `f_isr`) and rewrites the ISR context's calls; `crates/alloc` gives the ISR root a frame base **after** the main context's depth (disjoint region); `crates/sim` gains a `fire_interrupt()` test hook (push PC+1, PC = 4) — RETFIE already pops the return.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

## Global Constraints

- Build/test with `make exec CMD="cargo ..." …`; never bare `cargo`.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only.
- Unsupported constructs panic loudly, never silently miscompile.

## The lowering recipes (load-bearing design)

### The clang surface (probed, /tmp/m7probe/i2.ll)

- **ISR marker**: `define dso_local msp430_intrcc void @isr() #0` (the `msp430_intrcc` token in the return position; the `"interrupt"="N"` attribute is in the attributes group — the return token is the parse hook). `Func.isr: bool`.
- **SFR access**: `store volatile i8 85, ptr inttoptr (i16 6 to ptr), align 2` / `load ... ptr inttoptr (...)`. Parse `inttoptr (<ty> <k> to ptr)` → a literal pointer operand.

### The interrupt model

- The F877A has ONE interrupt vector at **word 0x0004** (the hardware pushes the return PC and clears GIE; PCLATH is NOT touched). **The vector IS the ISR entry** (no GOTO — a GOTO's target page would depend on the interrupted PCLATH, which is unknowable): the ISR's code starts at word 4.
- **Placement**: the isel emits the ISR **first** (before the other functions): after the 2-word `__start` (words 0–1), `.org 4` pads words 2–3, the ISR occupies 0x0004+ (page 0). The ISR body must fit page 0 (0x005–0x7FF) — the greedy page assignment gives it the front of page 0; a larger ISR panics loudly (ISRs are usually small).
- **Prologue** (the ISR's first instructions, all at word 4+):
  ```
  MOVWF 0x75          ; save W (MOVWF doesn't clobber W)
  SWAPF STATUS, W     ; STATUS -> W without touching STATUS
  MOVWF 0x76          ; save STATUS (nibble-swapped; restored with a swap)
  MOVF  PCLATH, W
  MOVWF 0x77          ; save PCLATH
  MOVF  FSR, W
  MOVWF 0x78          ; save FSR
  MOVLW 0x00
  MOVWF PCLATH        ; the ISR body runs in page 0 (its GOTOs need it)
  ```
- **Epilogue** (replacing the `ret`):
  ```
  MOVF 0x77, W
  MOVWF PCLATH        ; restore PCLATH
  SWAPF 0x76, W
  MOVWF STATUS        ; restore STATUS (swap-back)
  MOVF 0x78, W
  MOVWF FSR           ; restore FSR
  MOVF 0x75, W        ; restore W
  RETFIE
  ```
- **Within the ISR body**: the M11 PCLATH discipline applies normally (each CALL sets the target's page and restores to page 0 — the ISR's `PAGE(<cur_func>)` literal is 0). `0x75–0x78` are fixed common RAM (bank-independent; never used by locals per the M3 ruling; verify no collision with scratch 0x70 / retval 0x71–0x74).
- **The sim hook**: `fire_interrupt()` — push `pc + 1`, set `pc = 4`. RETFIE already pops the return (the sim's `0x0009`). GIE is unmodeled (the test controls the injection).

### SFR access (inttoptr)

- irparse: `inttoptr (<ty> <k> to ptr)` in a load/store pointer position → a literal pointer (the Load/Store `ptr` string form `"0x06"` — a new literal form distinct from `@global`/`%reg`).
- isel: a literal pointer load/store → direct `MOVF/MOVWF 0x06` (the SFR is bank-mirrored — no BANKSEL; the banking pass must not touch literal operands outside the GPR ranges — verify).
- Scope: load/store pointer positions only; `inttoptr` in call args → panic loudly (rare).

### The shared-function duplication (the approved ruling)

- After parsing (in `legalize`): build the call graph; find the ISR. The **ISR context** = the ISR + its transitive callees; the **main context** = main + its transitive callees. Every function in BOTH gets an `_isr` copy (a deep clone of the Func, renamed `{name}_isr`); every CALL inside the ISR context (the ISR and the copied functions) whose target is a duplicated function is rewritten to the `_isr` name. Non-shared functions are untouched.
- **The disjoint ISR region** (alloc): the ISR root's frame base = the max `depth_end` over the NON-ISR roots (the main context's total), not `bank0_start` — a preempted main's live frames must never overlap the ISR context's frames. The `_isr` copies are in the ISR root's chain (their frames live in the ISR region) ✓. The callgraph depth check applies per context (≤ 8 each).
- The duplicated functions in the canonical IR text (the driver's artifacts show `f_isr`) — a deliberate, diffable text change.

---

### Task 1: `ir` + `irparse` — the ISR marker and `inttoptr`

**Files:**
- Modify: `crates/ir/src/lib.rs`, `crates/irparse/src/lib.rs`
- Test: `crates/ir/tests/roundtrip.rs`, `crates/irparse/tests/parse_ll.rs` (extend)

**Interfaces:**
- Produces: `Func.isr: bool` (serialized as a marker in the canonical fn header — e.g. `fn isr() [isr] (…)`); `inttoptr (<ty> <k> to ptr)` in load/store pointer positions → the literal ptr form.

- [ ] **Step 1: Extend the failing tests** — parse_ll the i2.ll shapes: `define dso_local msp430_intrcc void @isr()` → `Func.isr == true`; `store volatile i8 85, ptr inttoptr (i16 6 to ptr)` → the Store ptr is the literal form; roundtrip the isr marker.
- [ ] **Step 2: Run to verify they fail** (ty_of panics on `msp430_intrcc`; parse_ptr on `inttoptr`).
- [ ] **Step 3: Implement** — the `msp430_intrcc` return handling (the ret type stays void; the isr flag set) + the `inttoptr` arm + the canonical serialization. (isel still panics on isr Funcs / literal ptrs — Task 2; minimal loud arms to keep the build green.)
- [ ] **Step 4: Run to verify they pass** + workspace builds.
- [ ] **Step 5: Commit** — `git commit -m "feat(ir,irparse): interrupt marker and inttoptr pointers"`.

---

### Task 2: `isel` — the ISR entry/prologue/epilogue + SFR access

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: for a `Func.isr`, `select` emits `.org 4` (pad words 2–3 after the 2-word `__start`), the save prologue, the body, the restore epilogue + `RETFIE` (instead of `RETURN`); the ISR is placed FIRST (the greedy assignment from word 4; page-0 fit, loud). Literal-pointer load/store → direct SFR access (no FSR, no BANKSEL).

- [ ] **Step 1: Extend the failing tests** — emitted-asm asserts: an isr Func emits `.org 4` + the 7-line save prologue + `RETFIE` + the restore epilogue (the exact save/restore order); a literal-pointer store (`store i8 %v $0x06`) → `MOVWF 0x06` (no FSR/BANKSEL); a literal-pointer load → `MOVF 0x06, W`. **SIM (load-bearing):** a module with main (loops, writes a global) + an isr (writes another global, calls a same-page helper); assemble + run; the e2e drives `step()` until a marker, calls `fire_interrupt()`, resumes — the ISR runs and `RETFIE` returns to the exact interrupted instruction (the interrupted computation completes with the correct result — context restored).
- [ ] **Step 2: Run to verify they fail** (isel panics on isr/literal ptrs).
- [ ] **Step 3: Implement** per the recipes.
- [ ] **Step 4: Run to verify they pass** — isel + workspace (non-ISR programs byte-identical — the ISR emission is gated on the flag).
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): interrupt entry, prologue and sfr access"`.

---

### Task 3: `sim` — the `fire_interrupt` hook

**Files:**
- Modify: `crates/sim/src/lib.rs`
- Test: `crates/sim/tests/sim.rs` (extend)

**Interfaces:**
- Produces: `Pic14::fire_interrupt(&mut self)` — push `pc + 1` (the return address), set `pc = 4`. (RETFIE already pops the return.) GIE unmodeled (the test controls the injection).

- [ ] **Step 1: Extend the failing tests** — a program whose word 4 is a marker (e.g. `MOVWF 0x75` … `RETFIE`): run a few steps, fire_interrupt, continue — the PC jumps to 4, the ISR runs, RETFIE returns to the pushed address (the interrupted code resumes).
- [ ] **Step 2: Run to verify it fails** (no API).
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run to verify it passes**.
- [ ] **Step 5: Commit** — `git commit -m "feat(sim): interrupt fire hook"`.

---

### Task 4: `legalize` + `alloc` — the shared-function duplication and the disjoint ISR region

**Files:**
- Modify: `crates/legalize/src/lib.rs`, `crates/alloc/src/lib.rs`
- Test: `crates/legalize/tests/legalize.rs`, `crates/alloc/tests/alloc.rs` (extend)

**Interfaces:**
- Produces: legalize duplicates every function reachable from BOTH the ISR and main contexts (`f` → `f_isr`, deep-cloned, renamed) and rewrites the ISR context's calls to the copies; alloc gives the ISR root a frame base = the max depth_end of the non-ISR roots (disjoint region), so a preempted main's frames never overlap the ISR context's.

- [ ] **Step 1: Extend the failing tests** — legalize: a module with main + isr both calling `helper` → the rewritten module has `helper` and `helper_isr`; the ISR's call targets `helper_isr`; main's call targets `helper`; a non-shared callee is NOT duplicated; the canonical text round-trips. alloc: main + isr + the copies — the ISR root's base is after the main context's depth (assert the addresses: the `_isr` copies' frames do not overlap any main-context frame).
- [ ] **Step 2: Run to verify they fail** (no duplication; roots share bank0_start).
- [ ] **Step 3: Implement** per the recipes.
- [ ] **Step 4: Run to verify they pass** — legalize + alloc + workspace (non-interrupt programs unchanged — the transform is gated on the ISR's existence).
- [ ] **Step 5: Commit** — `git commit -m "feat(legalize,alloc): duplicate interrupt-shared functions"`.

---

### Task 5: Acceptance — interrupt.c e2e + gpasm oracle

**Files:**
- Create: `crates/driver/tests/fixtures/interrupt.c`, `crates/driver/tests/interrupt_e2e.rs`
- Create: `crates/asm/tests/fixtures/interrupt.asm`, `crates/asm/tests/gpasm_interrupt.rs`

**Interfaces:**
- Consumes: Tasks 1–4 + `pic14_sim` + gpasm. Debug in the responsible stage; keep stage tests green.

- [ ] **Step 1: Write the failing acceptance program** — interrupt.c with a hand-computable result:

```c
#define PORTB (*(volatile unsigned char *)0x06)   // SFR access via inttoptr
volatile unsigned char out;
volatile unsigned char in;

__attribute__((noinline)) unsigned char bump(unsigned char x) { return (unsigned char)(x + 1); }

__attribute__((interrupt(0))) void isr(void) {
    PORTB = 0x55;                        // SFR write from the ISR
    out = bump(out);                     // shared helper (duplicated for the ISR)
}

void main(void) {
    out = in;                            // e.g. in = 0x10
    PORTB = 0x11;                        // SFR write from main
    out = bump(out);                     // shared helper (main's copy)
    out = (unsigned char)(out + 1);      // <- the interrupt fires during this stretch
    out = (unsigned char)(out + bump(2));
    PORTB = 0x22;
}
```

(The e2e fires the interrupt between two `step()`s — e.g. after `out = bump(out)` runs; the ISR writes PORTB=0x55 and bumps out to 0x11; the resumed main completes: 0x10 + 1 (main's bump) = 0x11, ISR: 0x11 → 0x12, main resumes: 0x12 + 1 + bump(2)=3 → 0x16. **Verify by hand from the exact emitted IR + the exact injection point** — the expected `out` depends on where the interrupt fires; the test documents the injection point and the traced value. The gpasm cross-check assembles the vector/RETFIE normally.)
- [ ] **Step 2: Write the acceptance test** — `interrupt_e2e.rs`: run the driver, parse the hex, drive the sim (step/run to the injection point, `fire_interrupt()`, resume), assert `out`, `PORTB`, and `halted()`. Debug in the responsible stage.
- [ ] **Step 3: Write the gpasm cross-check** — fixture the driver's `.asm` (the vector, the ISR prologue/epilogue, the `_isr` duplicate), assert our HEX == gpasm byte-for-byte + the same sim behavior (M6–M12 pattern).
- [ ] **Step 4: Run the full suite** — all green (probe → long → interrupt).
- [ ] **Step 5: Commit** — `git commit -m "test(e2e): interrupts and sfr access compile and run correctly"`.

---

## Self-review notes

- **Spec coverage:** M13 delivers phase 4's interrupt + SFR/device-access core (the approved duplicate-shared-functions ruling implemented). Remaining roadmap: phase 4's fuller device description (a header/register-name table), random testing (phase 6), soft-float (phase 7).
- **Correctness risks (verify by SIMULATION):** (1) the prologue/epilogue save/restore order (W last-in-first-out; STATUS via SWAPF both ways; PCLATH restored so the interrupted code's next CALL/GOTO works); (2) the ISR body's page-0 PCLATH discipline (the M11 restore literal is 0; the GOTOs stay in page 0); (3) the vector entry IS the ISR (no GOTO — the PCLATH-unknown problem); (4) the disjoint ISR region (a preempted main's live frames never overlap the ISR context's); (5) the duplication's call rewrite (every ISR-context call to a shared function → the `_isr` copy, transitively).
- **Deferred (later milestones):** GIE modeling in the sim (the injection hook suffices for tests); nested/priority interrupts (the F877A has one); the full device-header/register-name table; EEPROM/config words.
- **Contract:** `Func.isr`, the literal ptr form, the save-area addresses (0x75–0x78), the `.org 4` vector placement, the `_isr` naming, and the ISR-root disjoint base are the cross-crate contracts.
