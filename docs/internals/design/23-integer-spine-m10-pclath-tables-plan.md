# Integer Spine — Milestone 10: Const-Table PCLATH Page-Crossing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `const` (flash) RETLW tables grow beyond 256 bytes — the first code-model milestone. Today a table reader is `ADDLW LOW(table); MOVWF PCL` with **no PCLATH set** (a latent bug: the computed jump works only when the table sits in window 0); and `Global.size` is u8 (tables ≤ 255). M10: tables up to 512 bytes in **two 256-byte chunks**, each with an explicit `PCLATH` window set; `Global.size` widened to u16 for const globals; the whole program (code + tables) asserted to fit **page 0** (< 0x800) — multi-page CALL management is a later code-model milestone. Acceptance: a 300-byte table with indices in both chunks, pushed into a nonzero 256-byte window, runs correctly in the simulator and our HEX matches gpasm byte-for-byte.

**Architecture:** `crates/ir` + `crates/irparse` widen `Global.size` to u16 (const tables only; RAM globals keep the ≤ 255 assert); `crates/alloc` casts (RAM arrays ≤ 255, unchanged behavior); `crates/isel` re-emits every `__read_<name>` reader with the PCLATH window set (fixing the latent window bug), emits two-entry chunked readers for tables > 255 bytes, and adds a caller-side 16-bit index path (W = in-chunk index, temp-tested high bit selects the entry); the page-0 assert lands in the assembler (any program whose highest word address ≥ 0x800 panics loudly). The simulator already models PCLATH for `MOVWF PCL` (`crates/sim/src/lib.rs:231`), so no sim work is needed.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

## Global Constraints

- Build/test with `make exec CMD="cargo ..." …`; never bare `cargo`.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only.
- Unsupported constructs panic loudly, never silently miscompile.

## The lowering recipes (load-bearing design)

### The code model (this milestone's scope)

PIC16F877A: 8K words of flash = 4 pages of 2KB (0x800). `MOVWF PCL` sets `PC = (PCLATH & 0x1F) << 8 | W` (the sim's model); `CALL`/`GOTO` use `PCLATH<4:3> → PC<12:11>`. The stack holds the full PC, so `RETURN`/`RETLW` always return correctly regardless of PCLATH. **This milestone assumes the whole program fits page 0** (bits 4:3 = 0 throughout): functions are called with PCLATH<4:3> = 0, so a table reader may freely write PCLATH (its bits 4:3 stay 0 for page-0 tables) and the caller's subsequent code is undisturbed. The assembler **panics loudly** if the program's highest word address ≥ 0x800. (Multi-page programs — assigning functions to pages, setting PCLATH before every CALL — are the next code-model milestone.)

### The latent window bug (fixed here)

The current reader `__read_<name>: ADDLW LOW(name); MOVWF PCL` computes `PC = PCLATH:PCL` — with PCLATH at its reset/current value. If the table starts at e.g. 0x150 (window 1), the jump lands at 0x50–0x14F, not 0x150+. The M5 acceptance tables happened to land in window 0. **Every reader must set `PCLATH = HIGH(table)` before the computed jump** (2 instructions: `MOVLW HIGH(name); MOVWF PCLATH`). The M5-era const fixtures' readers gain these two lines; gpasm cross-checks stay byte-identical (both sides assemble the same new asm).

### Readers

**Small tables (size ≤ 255)** — reader shape (W = byte index, unchanged caller):
```
__read_<name>:
    MOVLW HIGH(<name>)      ; PCLATH = the table's 256-byte window
    MOVWF PCLATH
    ADDLW LOW(<name>)
    MOVWF PCL
<name>:                     ; chunk base
    RETLW ...  (size bytes)
```

**Large tables (256 ≤ size ≤ 511)** — two 256-byte chunks, two entries. The caller selects the entry (see below); each entry takes W = the **in-chunk** index (0..255):
```
__read_<name>:              ; W = in-chunk index (idx < 256)
    MOVLW HIGH(<name>)
    MOVWF PCLATH
    ADDLW LOW(<name>)
    MOVWF PCL
<name>:                     ; chunk 0 base
    RETLW ...  (256 bytes)
__read_<name>_hi:           ; W = in-chunk index (idx >= 256, i.e. idx - 256)
    MOVLW HIGH(<name>_1)
    MOVWF PCLATH
    ADDLW LOW(<name>_1)
    MOVWF PCL
<name>_1:                   ; chunk 1 base (label emitted at name + 256)
    RETLW ...  (size - 256 bytes)
```
Tables > 511 bytes: panic loudly (the selection chain generalizes; out of scope).

### The caller-side 16-bit index path

For large tables the GEP index is an i16 reg (clang: `zext` to i16, `getelementptr [300 x i8], ptr @table, i16 0, i16 %i`). The caller computes the in-chunk index (W) and the high bit, then calls the right entry. Single-scale-1-term shape (the common case; multi-term or const-only 16-bit indices into large tables → panic loudly for now):
```
    MOVF %r_lo, W
    ADDLW (k + off)         ; W = lo + k + off; C = carry into bit 8
    MOVWF 0x71              ; lo temp (retval_lo — no live retval here)
    MOVF %r_hi, W
    BTFSC STATUS, 0
    ADDLW 0x01              ; W = hi + carry
    MOVWF 0x70              ; hi temp (scratch — free at this point)
    MOVF 0x71, W            ; W = in-chunk index
    BTFSC 0x70, 0           ; idx >= 256?
    GOTO .hi
    CALL __read_<name>
    GOTO .done
.hi:
    CALL __read_<name>_hi
.done:
    MOVWF <dst>             ; the table byte (from the RETLW) -> dst
```
The small-table path (W = byte index, no temps, no branch) is unchanged. `0x70`/`0x71` are the fixed common-RAM scratch/retval bytes (bank-independent, never live at a const read — a const read is pure arithmetic between stores).

### Global.size → u16

`Global { size: u16 }`: irparse parses `[N x i8]` const globals with N ≤ 511 (the two-chunk bound; loud beyond); RAM globals keep N ≤ 255 (byte-addressed; alloc's `place_at` width is u8 — cast with a loud assert). `alloc` and `isel`'s `object_span` adapt mechanically. The canonical IR text does not carry size (unchanged — M5 ruling). `Global.addr` stays `Option<u8>` (separate deferred widening).

---

### Task 1: `ir` + `irparse` + `alloc` — `Global.size` u16

**Files:**
- Modify: `crates/ir/src/lib.rs`, `crates/irparse/src/lib.rs`, `crates/alloc/src/lib.rs`
- Test: `crates/irparse/tests/parse_ll.rs`, `crates/alloc/tests/alloc.rs` (extend)

**Interfaces:**
- Produces: `Global.size: u16`; irparse: const array `[N x i8]` with 256 ≤ N ≤ 511 parses (bytes = the literal), > 511 panics loudly; RAM arrays keep ≤ 255 (loud); alloc casts `size` to u8 for placement with a loud ≤ 255 assert (RAM-only — const globals are skipped).

- [ ] **Step 1: Extend the failing tests** — parse_ll: a `[300 x i8] constant c"..."` global (bytes length 300); a `[512 x i8] constant` panics loudly; a `[300 x i8] global` (RAM) panics loudly; alloc: a module with a const 300-byte table gets NO RAM address (const map entry only) and the RAM layout is unchanged.
- [ ] **Step 2: Run to verify they fail** (size ≤ 255 assert).
- [ ] **Step 3: Implement** — the u16 widening + asserts.
- [ ] **Step 4: Run to verify they pass** — irparse + alloc + workspace.
- [ ] **Step 5: Commit** — `git commit -m "feat(ir,irparse): 16-bit const table sizes"`.

---

### Task 2: `isel` + `asm` — the PCLATH readers and the page-0 assert

**Files:**
- Modify: `crates/isel/src/lib.rs`, `crates/asm/src/lib.rs`
- Test: `crates/isel/tests/isel.rs`, `crates/asm/tests/asm.rs` (extend)

**Interfaces:**
- Produces: every `__read_<name>` reader sets PCLATH (`MOVLW HIGH(<name>); MOVWF PCLATH`) before `ADDLW LOW(<name>); MOVWF PCL`; large tables (> 255 bytes) emit the two-entry chunked shape; the const-read caller path gains the 16-bit index path (single-term; W = in-chunk index, hi in 0x70, branch to `__read_<name>` / `__read_<name>_hi`; multi-term/const-only large-table indices panic loudly); the assembler panics loudly when the program's highest word address ≥ 0x800 (`assemble_file_to_hex` — it computes the words; page-0 only).

- [ ] **Step 1: Extend the failing tests** — isel: (a) a small const table's emitted reader now starts `MOVLW HIGH(t); MOVWF PCLATH; ADDLW LOW(t); MOVWF PCL`; (b) a 300-byte table's reader has both entries (`__read_t:` chunk-0 with `LOW(t)` and `__read_t_hi:` with `LOW(t_1)`) and the caller emits the lo-temp/hi-test/branch sequence; (c) multi-term large-table index panics. asm: a synthetic program whose words exceed 0x7FF panics loudly. **SIM (load-bearing):** (i) a small table placed in a NONZERO window (construct via hand-supplied addresses — e.g. arrange the table so LOW(t) ≥ 0x40 and the window ≠ 0... actually the window is determined by the code+table addresses in the assembled output; the SIM test assembles the emitted asm and can place a filler to push the table past 0x100) — a chunk-0 read must return the right byte WITH the PCLATH set (fails without it); (ii) a 300-byte table: reads at idx = 2 (chunk 0), idx = 256 (chunk 1 first byte), idx = 299 (chunk 1 last byte), idx = 290 — all asserted via pic14_sim.
- [ ] **Step 2: Run to verify they fail** (no PCLATH lines; the size-300 reader panics/doesn't exist).
- [ ] **Step 3: Implement** per the recipes; update the M5 const fixtures' expected reader asm (the 2 new PCLATH lines) and re-run the gpasm cross-checks.
- [ ] **Step 4: Run to verify they pass** — isel + asm + workspace (ptr_probe/array/structs e2e green with updated fixtures).
- [ ] **Step 5: Commit** — `git commit -m "feat(isel,asm): pclath const readers and page-0 bound"`.

---

### Task 3: Acceptance — const_table.c e2e + gpasm oracle

**Files:**
- Create: `crates/driver/tests/fixtures/const_table.c`, `crates/driver/tests/const_table_e2e.rs`
- Create: `crates/asm/tests/fixtures/const_table.asm`, `crates/asm/tests/gpasm_const_table.rs`

**Interfaces:**
- Consumes: Tasks 1–2 + `pic14_sim` + gpasm. Debug in the responsible stage; keep stage tests green.

- [ ] **Step 1: Write the failing acceptance program** — const_table.c with a 300-byte table (bytes 0..255 = 0x00..0xFF, bytes 256..299 = a distinctive pattern, e.g. 0x11,0x22,...,0x1E), a volatile `in`, a hand-computable `out`:

```c
const unsigned char table[300] = { /* 0..255 = 0x00..0xFF, 256..299 = 0x11,0x22,... */ };
volatile unsigned char out;
volatile unsigned char in;

void main(void) {
    out = table[in];                // chunk selection at runtime (in = 290 -> 0x1E)
    out = (unsigned char)(out + table[in & 3]);       // chunk 0: + table[2] = 2
    out = (unsigned char)(out + table[299]);          // constant idx, chunk 1: + 0x1E
    out = (unsigned char)(out + table[256]);          // chunk-1 first byte: + 0x11
}
```

(Expected for `in == 290`: 0x1E + 2 + 0x1E + 0x11 = 0x4F. **Verify by hand during the task** — if clang -O1 folds any piece (constant-folded `table[299]`, etc.), adjust to keep the same coverage (a runtime chunk-1 read, a chunk-0 read, a constant chunk-1 read, the chunk boundary) and recompute; document the exact emitted IR + final value in the test. **Push the table into a nonzero 256-byte window** — if the emitted .asm fixture shows the table at a window-0 address, add a filler const table (e.g. a small `const unsigned char pad[200]`) used once, so the main table lands past 0x100 — the PCLATH set is then load-bearing (the sim read fails without it). Keep the total program < 0x800 (the page-0 assert is loud).)
- [ ] **Step 2: Write the acceptance test** — `const_table_e2e.rs`: run the driver, simulate with `in = 290`, assert `out` and `halted()`. Debug in the responsible stage.
- [ ] **Step 3: Write the gpasm cross-check** — fixture the driver's `.asm`, assert our HEX == gpasm byte-for-byte + same sim `out` (M6–M9 pattern).
- [ ] **Step 4: Run the full suite** — all green (probe, overlay, banked, ptr_probe, array, scalar, structs, muldiv, banked_ptr, const_table).
- [ ] **Step 5: Commit** — `git commit -m "test(e2e): const tables past 256 bytes compile and run correctly"`.

---

## Self-review notes

- **Spec coverage:** M10 starts the code model — tables past 256 bytes with explicit PCLATH windows, plus the fix for the latent window bug. The next code-model milestone: multi-page programs (function-to-page assignment + PCLATH before every CALL).
- **Correctness risks (verify by SIMULATION):** (1) the PCLATH set on every reader (a table in a nonzero window fails without it — the acceptance pushes the table past 0x100 to make it load-bearing); (2) the chunk-1 entry's `LOW(name_1)`/`HIGH(name_1)` (the second chunk base label); (3) the caller's hi+carry computation (a low-byte `ADDLW k+off` carry must propagate into the chunk test — e.g. idx 0xFF0F+... within 511: idx = 0x0130 → lo 0x30, hi 0x01, no carry; idx = 0x00F0 + k=0x20 → lo 0x10, carry → hi 0x01 — the BTFSC STATUS,0; ADDLW 1 must fire); (4) the 0x70/0x71 temps are free at a const read (no live scratch/retval).
- **The M5 "byte-identical" property changes deliberately:** every const reader gains the 2-line PCLATH set. Fixtures/tests updated; gpasm cross-checks still byte-identical (same new asm both sides).
- **Contract:** the two-entry reader shape (`__read_<name>` / `__read_<name>_hi` with `name` / `name_1` chunk labels), the caller convention (W = in-chunk index; 0x70 = hi-temp; single-term large-table indices only), `Global.size: u16` (const ≤ 511, RAM ≤ 255), and the page-0 assert (< 0x800) are the cross-crate contracts.
