# Integer Spine — Milestone 4: Real Banking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Data exceeding bank 0 compiles correctly: `alloc` assigns addresses across all four banks, `isel` emits physical addresses as operands, `banking` inserts `BANKSEL` (`BSF`/`BCF STATUS, RP0/RP1`) when the current bank differs, and the simulator models bank selection (`RP1:RP0`) so a multi-bank program runs correctly. The milestone acceptance: a program whose globals + locals exceed 96 bytes (bank 0 + common) compiles with `BANKSEL`s and runs correctly in the bank-aware simulator, cross-checked against `gpasm`.

**Architecture:** The 877A data memory is four banks selected by `RP1:RP0` (STATUS bits 6:5). Direct operand `f` (0x00–0x7F) resolves: `0x00–0x1F` → bank-independent SFRs; `0x20–0x6F` → banked GPR at physical `f + bank*0x80`; `0x70–0x7F` → common RAM, mirrored in all banks. The map (`alloc`) carries **physical** addresses; `isel` emits them as operands; `banking` infers each operand's bank from its value, rewrites it to the 7-bit in-bank operand (`physical & 0x7F`), and inserts `BANKSEL` when the tracked current bank differs. The simulator resolves direct operands through `RP1:RP0` and indirect through `IRP`.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

**Spec:** [`docs/12-backend-design.md`](../12-backend-design.md) §2 stage 8 (banking) and §4 phase 2.

## Global Constraints

- Build/test with `nix develop --command cargo …`; never `apt install` toolchain deps.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only; GPL never linked.
- Text boundaries: stages communicate via text. The `alloc` map and the `isel`→`banking` `.asm` (with physical operands) are text artifacts.
- New files must be `git add`ed before `nix develop` sees them.
- Unsupported constructs panic loudly, never silently miscompile.

## The bank model (the load-bearing design)

- Physical address space: 512 bytes (`0x000–0x1FF`). Bank 0 GPR `0x20–0x6F`, bank 1 `0xA0–0xEF`, bank 2 `0x120–0x16F`, bank 3 `0x1A0–0x1EF` (0x190–0x19F is unimplemented RAM); common `0x70–0x7F` in all banks; SFRs `0x00–0x1F` bank-independent (our emitted SFRs — STATUS/FSR/PCL/PCLATH/INDF — are all mirrored).
- `bank = (STATUS >> 5) & 0x3` (RP1:RP0). `resolve(f) = if f in 0x70–0x7F { f } else if f in 0x20–0x6F { f + bank*0x80 } else { f }`.
- `alloc` assigns bank 0 first (`0x20–0x6F` after globals/scratch/retval/frames, then common `0x70–0x7F` for the overlay frames when bank 0 GPR is exhausted? **No — keep it simple: bank 0 GPR 0x20–0x6F, then bank 1 0xA0–0xEF, then bank 2, then bank 3; common RAM stays unused by locals (as in M3)**), and panics if total demand exceeds 320 bytes of GPR (4 × 80-byte regions).
- `banking` (the rewritten `assign_banks`): scan the `.asm`; for each file-register operand (byte-oriented and bit-oriented ops; **not** literal ops — the M3 skip list stays), infer bank from the operand value (`0x00–0x1F`/`0x70–0x7F` → no bank needed; `0x20–0x6F` → bank 0; `0xA0–0xEF` → bank 1; `0x120–0x16F` → bank 2; `0x1A0–0x1EF` → bank 3); if the needed bank differs from the tracked current bank, emit `BANKSEL <bank>` (as `BCF/BSF STATUS, RP0` and `BCF/BSF STATUS, RP1` for the two bits), update the tracked bank, and rewrite the operand to `physical & 0x7F`. The tracked bank is reset to UNKNOWN at every label (branch target), so the next banked operand there emits a full `BANKSEL` re-establishing both RP bits. Literal immediates are never banked.

---

### Task 1: `pic14-sim` — bank-aware memory model

**Files:**
- Modify: `crates/sim/src/lib.rs`
- Test: `crates/sim/tests/banking.rs` (new)

**Interfaces:**
- Consumes: the existing `Pic14` state (W, STATUS at `ram[0x03]`, FSR `0x04`, INDF `0x00`, PCL `0x02`, PCLATH `0x0A`, PC, stack).
- Produces: `ram` becomes `[u8; 512]` (public accessors `ram()`/`ram_mut()` keep their signatures, now `&[u8; 512]`); `read_f`/`write_f` resolve direct operands through `bank = (STATUS>>5)&3` per the model; INDF resolves through `IRP` (`bank = (STATUS>>7)&1` selects the upper/lower 256) with common mirroring; BSF/BCF on STATUS (the `BANKSEL` mechanism) work via the existing bit-op path. **Reset defaults to bank 0**, so all existing tests that preset `ram[0x20]`/`ram[0x21]` remain valid.

- [ ] **Step 1: Write the failing test** — `crates/sim/tests/banking.rs`:
  - bank isolation: set STATUS RP=01 (BSF STATUS, RP0 via `0x1423`... use the instruction words: `BSF 0x03,5` = 0x1400|(5<<7)|0x03 = 0x14A3; `BCF 0x03,6` = 0x1000|(6<<7)|0x03 = 0x1303), then `MOVWF 0x20` (writes physical 0xA0), `BCF STATUS, RP0` (back to bank 0), `MOVF 0x20, W` reads physical 0x20 (not 0xA0) — assert different values.
  - common mirror: write via `MOVWF 0x70` in bank 0, read `MOVF 0x70, W` in bank 1 (RP=01) — same value.
  - BANKSEL end-to-end: a program that `BSF STATUS, RP0`, writes a bank-1 cell, `BCF STATUS, RP0`, reads a bank-0 cell — assert both.
  - IRP indirect: FSR=0x20, IRP=1 (BSF STATUS, 7), `MOVWF INDF` writes physical 0x120; IRP=0 writes physical 0x20.
- [ ] **Step 2: Run to verify it fails** — `nix develop --command cargo test -p pic14-sim --test banking`.
- [ ] **Step 3: Implement** — the resolve model in `read_f`/`write_f`; grow `ram` to 512; keep INDF/PCL special handling. STATUS/FSR/PCL/PCLATH reads/writes resolve to physical `f` (bank-independent, `f < 0x20`).
- [ ] **Step 4: Run to verify it passes** — banking tests + the full sim suite (existing tests must still pass: reset = bank 0).
- [ ] **Step 5: Commit** — `git commit -m "feat(sim): bank-aware memory model"`.

---

### Task 2: `alloc` — assign across banks

**Files:**
- Modify: `crates/alloc/src/lib.rs`
- Test: `crates/alloc/tests/alloc.rs` (extend)

**Interfaces:**
- Produces: physical addresses in the map — globals and local frames fill bank 0 GPR (`0x20–0x6F`, after scratch/retval for locals), then bank 1 (`0xA0–0xEF`), bank 2 (`0x120–0x16F`), bank 3 (`0x1A0–0x1EF`); common RAM still unused by locals (M3 decision stands). Panic loudly if demand exceeds 320 bytes (`0x1EF` end).

- [ ] **Step 1: Extend the failing test** — a module with enough global bytes to overflow bank 0 (e.g. 90 i8 globals): assert the 81st global gets a physical address ≥ `0xA0`; and a module whose local frames overflow bank 0 (e.g. a function with ≥ 90 bytes of locals): assert the frame crosses into `0xA0+`. Assert the even-alignment for i16 still holds within each bank.
- [ ] **Step 2: Run to verify it fails** (currently everything packs into 0x20+ with no bank break).
- [ ] **Step 3: Implement** — two allocators (globals, frames) that each step through banks: `next` address; when `next` passes `0x6F`, jump to `0xA0`; when past `0xEF`, `0x120`; when past `0x16F`, `0x1A0`; panic past `0x1EF`. (Physical addresses; i16 even-aligned within each bank region.)
- [ ] **Step 4: Run to verify it passes**.
- [ ] **Step 5: Commit** — `git commit -m "feat(alloc): assign addresses across all four banks"`.

---

### Task 3: `banking` — BANKSEL insertion

**Files:**
- Modify: `crates/banking/src/lib.rs`
- Test: `crates/banking/tests/banking.rs` (extend)

**Interfaces:**
- Consumes: the `.asm` text with **physical** operands (isel emits the map addresses as-is).
- Produces: `assign_banks(&asm) -> String` — for each file-register operand (byte ops + bit ops; literal ops skipped per the M3 skip list), infer the bank from the operand value, insert `BANKSEL` when the tracked current bank differs, rewrite the operand to `physical & 0x7F`. `BANKSEL <n>` emits `BCF/BSF STATUS, RP0` and `BCF/BSF STATUS, RP1` for the two RP bits (RP0 = STATUS bit 5, RP1 = bit 6; only the bits that change are emitted). The pass tracks the current bank as it scans (initial bank 0; `BCF/BSF STATUS, RP0/RP1` instructions it emits or encounters update the tracked bank). `0x00–0x1F` (SFRs) and `0x70–0x7F` (common) operands need no BANKSEL and no rewrite (common stays `0x70–0x7F`; SFRs stay as-is).

- [ ] **Step 1: Extend the failing test** — hand-written `.asm` with `MOVF 0xA0, W` and `MOVF 0x20, W` (bank 1 then bank 0): assert the output contains `BSF STATUS, RP0` (or equivalent) before the 0xA0 instruction, the operand rewritten to `0x20`, and a `BCF STATUS, RP0` before returning to bank 0; `MOVF 0x70, W` (common) and `BTFSC STATUS, 2` (SFR) get no BANKSEL; consecutive same-bank operands get no redundant BANKSEL.
- [ ] **Step 2: Run to verify it fails**.
- [ ] **Step 3: Implement** — the scan; keep the literal-op skip (M3) and the ≥0x80 rejection for *in-bank* operands that were never rewritten (defensive: a physical operand `0x20–0x6F` in bank 0 needs no rewrite; `0x80–0x9F` SFR-range operands are out of scope for our emissions and still rejected).
- [ ] **Step 4: Run to verify it passes** — banking tests + `nix develop --command cargo test --workspace` (the existing overlay/probe e2e use only bank 0, so no BANKSEL is inserted and their `.asm` is unchanged).
- [ ] **Step 5: Commit** — `git commit -m "feat(banking): insert BANKSEL across banks"`.

---

### Task 4: Acceptance — a multi-bank program runs correctly

**Files:**
- Create: `crates/driver/tests/fixtures/banked.c`, `crates/driver/tests/banked_e2e.rs`
- Create: `crates/asm/tests/fixtures/banked.asm`, `crates/asm/tests/gpasm_banked.rs`

**Interfaces:**
- Consumes: the full pipeline (Tasks 1–3) and `pic14_sim`.

- [ ] **Step 1: Write the failing acceptance program** — `banked.c`: enough volatile i8 globals to exceed bank 0 (e.g. 90+ globals, `volatile unsigned char g0 … g89;`), plus `main` that writes then reads every global (volatile stores/loads survive `-O1`) and sums them into `out`:

```c
volatile unsigned char g0;  /* … g89: 90 globals = 90 bytes > 80 bank-0 GPR */
volatile unsigned char in;
volatile unsigned char out;
void main(void) {
    g0 = 1; g1 = 2; /* … */ g89 = 90;
    unsigned int s = 0;
    s += g0; s += g1; /* … */ s += g89;
    out = (unsigned char)s;
}
```

(Write the file with a generator loop or expand it; the exact assignments don't matter — the acceptance is: the program needs > 96 bytes of RAM, `banking` inserts BANKSELs, and the sim runs it correctly. Hand-compute the expected `out`: sum 1..90 = 4095, `4095 & 0xFF = 0xFF`... verify: 4095 = 0xFFF, low byte 0xFF — so `out = 255`. Recompute when finalizing the program.)

- [ ] **Step 2: Write the acceptance test** — `banked_e2e.rs` runs the driver, simulates (no presets needed beyond the program's own writes), asserts `out` equals the hand-computed value and `halted()`. Also assert (inspect the `.asm` via the driver pipeline or by running the stage binaries) that the emitted `.asm` contains at least one `BSF STATUS, RP0`-style BANKSEL (i.e., banking actually engaged — not a bank-0-only program). 
- [ ] **Step 3: Run to verify it fails, then make it pass** — debug in the responsible stage (likely banking operand handling or the sim resolve); keep stage tests green. The `gpasm_banked.rs` cross-check mirrors the M3 pattern: fixture the driver's `.asm` for `banked.c`, assert our HEX equals gpasm byte-for-byte and the sim run gives the same `out`.
- [ ] **Step 4: Run the full suite** — `nix develop --command cargo test` all green (probe, add.c, overlay, banked e2e).
- [ ] **Step 5: Commit** — `git commit -m "test(e2e): multi-bank program compiles and runs with BANKSEL"`.

---

## Self-review notes

- **Spec coverage:** milestone 4 implements spec §2 stage 8 (banking) end-to-end — physical addresses, BANKSEL insertion, bank-aware simulation — closing the integer spine's core. The bank-assignment strategy (fill banks sequentially) is the conservative baseline; BANKSEL minimisation (the NP-hard dataflow) is a later optimization, and the sequential scan already avoids redundant switches on straight-line code.
- **Deferred (later milestones, panic loudly until then):** common-RAM imaginary registers; BANKSEL minimisation (NP-hard, CASES'06 2-approx — a later milestone); per-SFR bank maps (interrupts/SFR headers, phase 4 — our emitted SFRs are all bank-independent); pointers/GEP (phase 3).
- **Correctness notes for the implementer:** the sim's `resolve` must be exact per the model (SFR bank-independence, common mirroring, `f + bank*0x80`); the banking scan must track the bank correctly through its own BANKSEL emissions and never rewrite a literal; isel needs no change (it emits map addresses as-is — verify the probe/overlay `.asm` output is unchanged since their data is all bank 0). The `ram` size change (256→512) is internal to `Pic14`; the accessor signatures stay, so the e2e presets (`ram_mut()[0x20]`) remain valid (bank 0 at reset).
- **Type consistency:** `pic14_sim::{Pic14, parse_hex, ram_mut}`, `alloc::allocate`, `banking::assign_banks` — signatures stable; the `.asm` operand convention (physical addresses pre-banking, 7-bit post-banking) is the banking contract.
