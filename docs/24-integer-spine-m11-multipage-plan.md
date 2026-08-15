# Integer Spine — Milestone 11: Multi-Page Code Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** programs grow past 2KB — functions span the PIC16F877A's four 2KB pages (8K words total). Today the assembler asserts the whole program fits page 0 (< 0x800); M11 removes that by: (1) assigning each function to a page (a function must fit within one page — its intra-function GOTOs need a stable page; a function > 0x800 panics loudly), (2) emitting `PCLATH` set/restore discipline around **every CALL** (function calls, const-table `__read_*` calls, and the `__start: CALL main`), (3) a `PAGE(label)` assembler resolution (the target's `addr >> 11 << 3`, i.e. the PCLATH<4:3> literal). The M10 const readers already manage PCLATH internally, so tables may span pages freely. Acceptance: a > 2KB program with functions across three pages, cross-page calls, and a const table in a later page runs correctly and our HEX matches gpasm byte-for-byte.

**Architecture:** `crates/asm` resolves `PAGE(label)` (like `LOW()`/`HIGH()`) and asserts the program fits the device's 0x2000 words; `crates/isel` tracks the emitted word address (1 word per instruction line; `.org`/`.align`/labels are padding/0), greedily assigns functions to pages (emitting `.org <page base>` before a function that doesn't fit the current page's remainder), and wraps every CALL in `MOVLW PAGE(<target>); MOVWF PCLATH` / `MOVLW PAGE(<cur_func>); MOVWF PCLATH`; `crates/peephole` elides redundant PCLATH writes (same literal — the common same-page-call case). The simulator already models PCLATH for CALL/GOTO/`MOVWF PCL` (verified in M10), so multi-page SIM tests work unchanged.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

## Global Constraints

- Build/test with `nix develop --command cargo …`; never bare `cargo`.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only.
- Unsupported constructs panic loudly, never silently miscompile.

## The lowering recipes (load-bearing design)

### The PCLATH discipline (from M10's code model)

- `CALL`/`GOTO` use `PCLATH<4:3> → PC<12:11>`; `MOVWF PCL` uses `PCLATH<4:0> → PC<12:8>`; `RETURN`/`RETLW` pop the full PC (page-independent). PCLATH is **not** modified by CALL/GOTO/RETURN.
- **Every function's entry runs with PCLATH<4:3> = its own page** — set by the caller (before the CALL) or by `__start` (for `main`).
- **The caller discipline**: before every CALL, `MOVLW PAGE(<target>); MOVWF PCLATH`; immediately after, `MOVLW PAGE(<cur_func>); MOVWF PCLATH` (restore). This keeps intra-function GOTOs correct (they always run with the function's own page). The restore literal is a fixed constant per function.
- **Functions must fit one page** (intra-function GOTOs need a single stable page): a function whose emitted size > 0x800 panics loudly. Greedy page assignment in module order: emit `.org <next page base>` before a function that doesn't fit the current page's remainder (the `.org` pads with 0x0000 words — the assembler already supports it); pages 0–3; beyond → panic loudly.
- **Tables are unconstrained** (their readers set PCLATH internally — the full window, M10): a table may span a page boundary (the chunk-1 computed goto's `HIGH(t_1)` covers the next page's window). Only the `__read_*` reader's own location matters — the caller sets `PAGE(__read_t)` (resolved from the reader's actual address) before the CALL.
- **`__start: MOVLW PAGE(main); MOVWF PCLATH; CALL main; SLEEP`** — no restore needed (program ends).
- **Whole-program bound**: the assembler's page-0 assert (M10) becomes a device-flash assert: highest word address < 0x2000 (8K words), loud.

### The `PAGE(label)` assembler resolution

`MOVLW PAGE(<label>)` → the literal `(<label_addr> >> 11) << 3` (bits 4:3 = the page, bits 2:0 = 0). Resolved in pass 1 like `LOW()`/`HIGH()`; a missing label panics loudly. (For `main` in page 1: `PAGE(main)` = 0x08.)

### isel's address tracking (the page-assignment pre-computation)

isel emits each function into its own `Vec<String>`; the word size = the number of instruction lines (our asm is 1 word/line; labels/directives are 0; `.align N` pads to the boundary). The running address: functions (sizes + `.org` padding), then tables (tracking their `.align` padding so the emitted addresses are consistent — the assembler resolves labels, but isel needs the running address ONLY for the function-page decisions). Emission stays linear: before a function that would cross the current page's end, emit `.org <next base>` (pad), update the running address, emit the function, advance.

### The peephole elision (redundant PCLATH writes)

After the always-emit discipline, most calls are same-page (the restore equals the pre-set value). The peephole tracks the last PCLATH literal across the asm text (updating on every `MOVWF PCLATH`; CALL/GOTO/labels do not write PCLATH) and **drops a `MOVLW k; MOVWF PCLATH` pair when the tracked value == k**. Sound because nothing else writes PCLATH and CALL/GOTO leave it unchanged; the M10 readers' window sets are kept when `HIGH(table) != PAGE(…)`. Tests pin the elision (same-page call → the restore pair disappears; cross-page → both kept; a reader's window set kept when different).

---

### Task 1: `asm` — `PAGE(label)` + the device-flash bound

**Files:**
- Modify: `crates/asm/src/lib.rs`
- Test: `crates/asm/tests/asm.rs` (extend)

**Interfaces:**
- Produces: `MOVLW PAGE(<label>)` resolves to `(addr >> 11) << 3`; the M10 `< 0x800` assert becomes `< 0x2000` (device flash), loud; `LOW()`/`HIGH()`/`.org`/`.align`/`.table` unchanged.

- [ ] **Step 1: Extend the failing tests** — `MOVLW PAGE(x)` for labels at 0x000/0x800/0x1000/0x1800 resolves to 0x00/0x08/0x10/0x18; a missing label panics; a synthetic program whose highest word ≥ 0x2000 panics loudly (the old 0x800 assert must NOT fire for a 0x1000-located program — update the M10 page-0 test to the new bound).
- [ ] **Step 2: Run to verify they fail** (no PAGE resolution; the old bound fires).
- [ ] **Step 3: Implement** — the resolution (pass 1 symbol table → the literal, mirroring LOW/HIGH) + the bound change.
- [ ] **Step 4: Run to verify they pass** — asm tests + workspace.
- [ ] **Step 5: Commit** — `git commit -m "feat(asm): resolve PAGE() and bound to device flash"`.

---

### Task 2: `isel` — address tracking, page assignment, PCLATH discipline

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: isel tracks the running word address (per-function line counts + `.org`/`.align` padding) and assigns functions to pages greedily (emit `.org <base>` before a function that doesn't fit the current page's remainder; a function > 0x800 panics loudly; beyond page 3 → panic); every CALL (`emit_call`'s `CALL func`, the const readers' `CALL __read_<name>`, and `__start: CALL main`) is wrapped in `MOVLW PAGE(<target>); MOVWF PCLATH` … `CALL` … `MOVLW PAGE(<cur_func>); MOVWF PCLATH` (the `__start` omits the restore); the restore literal is the current function's page. The M10 acceptance fixtures' emitted asm gains the PCLATH pairs (same-page → elided later by Task 3; for now always emitted — the fixtures/tests updated; gpasm cross-checks re-run).

- [ ] **Step 1: Extend the failing tests** — a two-function module: `main` calls `helper` → the emitted asm has `MOVLW PAGE(helper); MOVWF PCLATH; CALL helper; MOVLW PAGE(main); MOVWF PCLATH`; a const table read → the `__read_` CALL has the same discipline; a synthetic module with a function > 0x800 words panics loudly. **SIM (load-bearing):** a multi-page module — pad functions so `helper` lands in page 1 (its emitted size + main's size push the `.org 0x800`); assemble + run: main calls helper (cross-page), helper does an intra-function GOTO branch (proving the restore discipline), a const table lands in page 1 (the reader's computed goto + the caller's PAGE(__read_) set) — assert the results via pic14_sim.
- [ ] **Step 2: Run to verify they fail** (no PCLATH emissions today; the page-0 assert fires).
- [ ] **Step 3: Implement** per the recipes (the address tracker, the greedy assignment, the discipline); remove the isel-side reliance on the old page-0 bound (the asm-side bound changed in Task 1).
- [ ] **Step 4: Run to verify they pass** — isel + workspace (all existing e2e green — their small programs land in page 0, the discipline is trivially satisfied).
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): multi-page functions with pclath call discipline"`.

---

### Task 3: `peephole` — elide redundant PCLATH writes

**Files:**
- Modify: `crates/peephole/src/lib.rs`
- Test: `crates/peephole/tests/peephole.rs` (extend)

**Interfaces:**
- Produces: the tracked-literal elision (see the recipe): `MOVLW k; MOVWF PCLATH` dropped when the tracked PCLATH literal == k (tracked across the text; CALL/GOTO/labels don't write PCLATH).

- [ ] **Step 1: Extend the failing tests** — `MOVLW 0x08; MOVWF PCLATH; CALL f; MOVLW 0x08; MOVWF PCLATH` → the trailing pair elided (same-page call); `MOVLW 0x08; MOVWF PCLATH; CALL f; MOVLW 0x00; MOVWF PCLATH` → both kept (cross-page); a reader's `MOVLW HIGH(t); MOVWF PCLATH` with `HIGH(t) == 0x08` after a `MOVLW 0x08; MOVWF PCLATH` → the reader's set elided (correct — the window equals the tracked value); with `HIGH(t) == 0x09` → kept.
- [ ] **Step 2: Run to verify they fail** (pass-through today).
- [ ] **Step 3: Implement** per the recipe.
- [ ] **Step 4: Run to verify they pass** — peephole + workspace (the driver pipeline runs the peephole last — the multi-page asm shrinks).
- [ ] **Step 5: Commit** — `git commit -m "feat(peephole): elide redundant pclath writes"`.

---

### Task 4: Acceptance — multi_page.c e2e + gpasm oracle

**Files:**
- Create: `crates/driver/tests/fixtures/multi_page.c`, `crates/driver/tests/multi_page_e2e.rs`
- Create: `crates/asm/tests/fixtures/multi_page.asm`, `crates/asm/tests/gpasm_multi_page.rs`

**Interfaces:**
- Consumes: Tasks 1–3 + `pic14_sim` + gpasm. Debug in the responsible stage; keep stage tests green.

- [ ] **Step 1: Write the failing acceptance program** — multi_page.c that exceeds 2KB with a hand-computable `out`:

```c
volatile unsigned char out;
volatile unsigned char in;
const unsigned char table[300] = { /* 0..255 = 0x00..0xFF, 256..299 = 0x11..0x1E */ };

__attribute__((noinline)) unsigned char f1(unsigned char x) {  // big: unrolled arithmetic
    unsigned char r = x;
    r = (unsigned char)(r + x); r = (unsigned char)(r * 3); r = (unsigned char)(r / 5);
    /* ... repeat ~20 times with different constants so f1 is ~300+ words ... */
    return r;
}
__attribute__((noinline)) unsigned char f2(unsigned char x) { /* similarly big, calls f1 */ }
__attribute__((noinline)) unsigned char f3(unsigned char x) { /* big, calls f2, reads table */ }

void main(void) {
    out = f3(in);                    // cross-page call chain + table in a later page
    out = (unsigned char)(out + f1(table[in & 3]));  // table read + cross-page call
}
```

(Functions padded (unrolled `r = (r + x) / k` sequences with distinct constants — all within the supported surface) so the total program exceeds 0x800 with functions in pages 1–2 and the table past 0x1000; `out` hand-computable and documented. **Verify by hand during the task** — recompute from the exact emitted IR; the peephole may shrink the asm (elided PCLATH pairs) — the WORD count that matters for page placement is the pre-peephole isel output (isel's tracker runs before the peephole — check the page assignment is consistent with the FINAL assembled addresses: the elision doesn't change addresses (it drops 2-word pairs, shifting subsequent addresses! A dropped pair shifts the following addresses by 2 → the `.org` padding was computed pre-elision → the labels move → the pages could shift!). **CRITICAL: the page assignment must be consistent with the final layout** — either the tracker accounts for the elision (hard) or the elision must NOT change page membership (a 2-word shift can't change a function's page unless it straddles a boundary — the greedy `.org` padding absorbs shifts since the `.org` target is absolute: after an elision, the running address is 2 less, the `.org 0x800` still fires when the accumulated size crosses — the FUNCTION's page = its final address >> 11 — a 2-word shift could move a function from 0x7FE to 0x7FC (same page) or across... the `.org` guarantees the function starts at a page base — the shift only matters if it moves the CROSSING point: elisions BEFORE the crossing reduce the pre-padding size — the `.org` fires at the same crossing decision... the PAGE of each function is decided by isel's tracker; the final addresses (after elision) could differ by a few words but the `.org` bases are ABSOLUTE (0x800/0x1000) — a function assigned to page 1 starts at 0x800 regardless — its final address is 0x800 + (its position within page 1's post-elision layout) — the page membership is STABLE (the `.org` pins it). Only the FIRST page's functions (before any .org) could shift across the 0x800 boundary if elisions push them... elisions only REMOVE words → the first-page functions shrink → never cross INTO page 1. And a function assigned to page 1 could shrink to fit... no — it starts at 0x800 by the .org ✓. So the assignment is stable under elision. Document this reasoning in the test.)
- [ ] **Step 2: Write the acceptance test** — `multi_page_e2e.rs`: run the driver, simulate, assert `out` and `halted()`. Debug in the responsible stage.
- [ ] **Step 3: Write the gpasm cross-check** — fixture the driver's `.asm`, assert our HEX == gpasm byte-for-byte + same sim `out` (the `.org`/`.align`/`.table`/`PAGE()` all NOP/literal-translated for gpasm as in M10 — the fixture must include the same translations) (M6–M10 pattern).
- [ ] **Step 4: Run the full suite** — all green (probe → multi_page).
- [ ] **Step 5: Commit** — `git commit -m "test(e2e): multi-page programs compile and run correctly"`.

---

## Self-review notes

- **Spec coverage:** M11 completes the code model — 8K-word programs with cross-page calls and tables. Remaining roadmap: 32-bit `long` (phase 5), interrupts+SFR+device description (phase 4), random testing (phase 6), soft-float (phase 7).
- **Correctness risks (verify by SIMULATION):** (1) the restore-after-call discipline (a cross-page call followed by an intra-function GOTO must still branch within the caller's page); (2) `__start`'s PCLATH set for a main in a nonzero page; (3) a table in a later page (the reader's full-window set + the caller's `PAGE(__read_)`); (4) the greedy page assignment + `.org` padding (the elision-stability reasoning above); (5) the peephole elision must never drop a needed set (the tracked-literal rule — a reader's window set kept when different).
- **M10 fixture churn:** every call site gains the PCLATH pairs; the fixtures/tests update; gpasm cross-checks re-run. The peephole (Task 3) then collapses the same-page pairs, so the final acceptance asm is close to the M10 size for same-page calls.
- **Deferred (later milestones):** bin-packing page assignment (the greedy `.org` wastes page tails); PAGESEL-style branch optimizations; interrupt entry (needs the ISR in a known page — phase 4).
- **Contract:** `PAGE(label)` resolution, the always-set/always-restore discipline, the per-function-fit rule (≤ 0x800), the device bound (< 0x2000), and the peephole tracked-literal rule are the cross-crate contracts.
