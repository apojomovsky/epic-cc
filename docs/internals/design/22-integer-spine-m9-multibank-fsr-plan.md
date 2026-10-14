# Integer Spine — Milestone 9: Multi-Bank FSR+IRP Arrays Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** FSR-based indirect access (pointers into RAM arrays, structs, byval/sret targets) reaches **all four banks**. Today `emit_fsr_to`/`emit_fsr_indirect` assert every FSR base ≤ 0xFF (bank 0/1 only, and even bank-1 bases are technically wrong without IRP discipline); M9 makes indirect access correct across the whole 0x20–0x1EF GPR space using the PIC16 **IRP bit** (STATUS bit 7): `IRP = base bit 8`, `FSR = base & 0xFF`. Acceptance: a program with arrays in banks 1–3, dynamically indexed through pointers, runs correctly in the simulator and our HEX matches gpasm byte-for-byte.

**Architecture:** purely `crates/isel` (the FSR setup sites) + acceptance tests. No ir/irparse/alloc changes: globals and locals already step through all four banks (M4); the FSR window constraint is enforced **loudly at isel time** (an FSR-accessed object must fit entirely within one of the four GPR windows — crossing an SFR hole silently mis-addresses, so it panics). The simulator already models INDF-via-IRP (`crates/sim/src/lib.rs:128` — `indirect_addr`: IRP → base 0x100, FSR 0x70-0x7F common), so no sim work is needed.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

## Global Constraints

- Build/test with `make exec CMD="cargo ..." …`; never bare `cargo`.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only.
- Unsupported constructs panic loudly, never silently miscompile.

## The lowering recipes (load-bearing design)

### The FSR+IRP model

PIC16F877A GPR banks: bank 0 `0x20–0x7F`, bank 1 `0xA0–0xEF`, bank 2 `0x120–0x16F`, bank 3 `0x1A0–0x1EF` (alloc's `region_for`). The SFR holes `0x80–0x9F` and `0x170–0x19F` are **not** addressable GPR. INDF resolution (sim and silicon): `IRP=0` → `0x000 + FSR`, `IRP=1` → `0x100 + FSR`; FSR `0x70–0x7F` is the mirrored common region (bank-independent).

For an address `A` in GPR space:
- `IRP = (A >> 8) & 1` (bit 8)
- `FSR value = A & 0xFF`
- **Window constraint:** the whole accessed object must fit inside one of the four GPR windows `[0x20,0x80)`, `[0xA0,0xF0)`, `[0x120,0x170)`, `[0x1A0,0x1F0)` (the common region 0x70–0x7F is inside the first window and is fine). `A + size ≤ window_end(A)`, else **panic loudly** (a silent cross-hole access mis-addresses into SFRs).

### The emission changes (all in `crates/isel`)

1. **`emit_fsr_to(base_addr, k, terms, byte_off)`** (static bases — globals, byval-param slots, alloca slots):
   - New signature gains the object's `span` (Global.size for `Base::Global`, param width / alloca size for `Base::Slot` — isel looks these up from `self.m`).
   - Emit the IRP set first: `BCF STATUS, 7` (base < 0x100) / `BSF STATUS, 7` (base ≥ 0x100).
   - FSR literal becomes `(base_addr + k + byte_off) & 0xFF` (the ADDLW literal is 8-bit by construction).
   - Replace the `base_k <= 0xFF` assert with the window check on `base_addr + span` (the terms are runtime values bounded by span − 1).
   - The single-scale-1-term fast path keeps the M5 shape (`MOVF %r,W; ADDLW k; MOVWF FSR`) — **plus the IRP set before it**. The "byte-identical to M5" property is now "byte-identical plus the IRP set"; the M5/M7/M8 FSR tests and the ptr_probe/array/structs fixtures must be updated with the `BCF STATUS, 7` line, and gpasm cross-checks still hold (both sides assemble the same new asm).
   - **IRP is emitted on every FSR setup, including bank-0/1 bases** — a prior bank-2/3 access leaves IRP=1, so skipping the set would mis-address. (STATUS bit 7 does not disturb the banking pass's BANKSEL logic — the sim's `bank_base` masks `STATUS<6:5>` for direct accesses, so direct and indirect paths are independent.)
2. **`emit_fsr_indirect(slot_addr, k, terms, byte_off)`** (sret params — the slot holds the target address):
   - IRP from the stored address's high byte: `BTFSC <slot+1>, 0; BSF STATUS, 7; BTFSS <slot+1>, 0; BCF STATUS, 7` before the FSR computation.
   - FSR = `[slot_lo] + k + off + terms` unchanged (the runtime base is unknown here; the k+off ≤ 0xFF literal assert stays).
3. **Sret caller store** (the `arg.sret` arm in `emit_call`): relax `assert!(addr <= 0xFF)` → the window check with the target object's size (a `Val::Global` → `Global.size`; a `Val::Reg` alloca → the alloca's size from the caller's blocks): `addr + object_size ≤ window_end(addr)`, loud. The `MOVLW LOW/HIGH` address store already emits both bytes (bank-2/3 targets already store the correct hi byte).
4. **Object-span lookups:** a small helper resolves `(Base) → span`: `Base::Global(name)` → `Global.size` from `self.m.globals`; `Base::Slot(name, _)` → the param width (`{cur_func}`'s `Param.width`) or the alloca size (an `Inst::Alloca` whose dst == name in `{cur_func}`'s blocks). Missing → panic loudly.

### What does NOT change

- Direct (non-FSR) accesses to banked globals/locals: already BANKSEL'd (M4), untouched.
- The const (flash) RETLW path: index-based, no FSR, untouched.
- The runtime routines (M8): direct MOVF/MOVWF with the ≤ 0x7F bank-0 assert (a BANKSEL-skip-hazard bound, unrelated to FSR) — untouched.
- alloc: nothing (placement already spans banks; the FSR window constraint is enforced at isel).

---

### Task 1: `isel` — the FSR window/IRP machinery (static bases)

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: the window helper (`fn fsr_window(base_addr, span) -> (irp: bool, base_lo: u8)` — loud panic outside GPR space or crossing a window end), `emit_fsr_to` rework (span parameter, IRP set, `& 0xFF` literal), `emit_ptr_setup` passes spans (via the object-span helper), and every existing FSR-using test/fixture updated with the `BCF STATUS, 7` line.

- [ ] **Step 1: Extend the failing tests** — new tests: (a) FSR setup for bases in each of the four banks: `%p = gep @g4 +0 +1*%i` with `g4` at 0x1A0 → `BSF STATUS, 7` + `MOVF %i,W; ADDLW 0xA0; MOVWF FSR`; a bank-0 base → `BCF STATUS, 7`; (b) a window-crossing object panics loudly (global at 0x78 with size 16 → crosses the 0x80 hole; global at 0x150 size 32 → crosses 0x170); (c) an object-span lookup (byval param slot base at 0x120 with width 16 → BSF + correct FSR). Update the existing FSR tests with the IRP line (assert-both).
- [ ] **Step 2: Run to verify they fail** (current code: `BSF`/`BCF STATUS, 7` absent; window crossing doesn't panic).
- [ ] **Step 3: Implement** per the recipes; update the M5/M7 FSR tests + the pointer fixtures' expected asm (the IRP line) and re-run the gpasm cross-checks (byte-identical to gpasm still holds — same new asm both sides).
- [ ] **Step 4: Run to verify they pass** — isel + workspace (ptr_probe/array/structs e2e green with the updated fixtures). **SIM tests (load-bearing):** assemble and run modules with dynamic-indexed arrays in bank 1 (0xA0), bank 2 (0x120), and bank 3 (0x1A0) — write + read back, assert the values (e.g. `arr[3] = 0x11; out = arr[3]` per bank, plus an interleaved sequence that alternates bank-0 and bank-2 accesses to prove the IRP is re-set per access).
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): multi-bank FSR via IRP"`.

---

### Task 2: `isel` — indirect (sret) IRP + sret caller window assert

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: `emit_fsr_indirect` sets IRP from the stored address's hi byte; the sret caller arm replaces `addr <= 0xFF` with the window check on `addr + object_size` (object size from the global or alloca lookup); the sret `MOVLW HIGH` store already emits both bytes (unchanged).

- [ ] **Step 1: Extend the failing tests** — (a) emitted-asm assert: an sret call whose target is an alloca at 0x120 → the caller stores `MOVLW 0x20; MOVWF pa; MOVLW 0x01; MOVWF pa+1` and the callee's indirect store emits the `BTFSC pa+1, 0; BSF STATUS, 7; BTFSS pa+1, 0; BCF STATUS, 7` IRP dance; (b) an sret target in a window-crossing position panics loudly (alloca at 0x130 size 16 → crosses 0x170); (c) an out-of-GPR sret target panics. **SIM test:** a full sret call end-to-end with the target alloca in bank 2 (hand-supplied addrs): the callee writes both struct bytes through the indirect pointer, the caller reads them back — assert both values (this proves the IRP-from-hi-byte path through the simulator's INDF model).
- [ ] **Step 2: Run to verify they fail** (no IRP in the indirect path; the ≤0xFF assert fires on a 0x120 target).
- [ ] **Step 3: Implement** per the recipes.
- [ ] **Step 4: Run to verify they pass** — isel + workspace (existing sret tests updated only where the emitted asm gains the IRP lines).
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): sret targets in any bank via IRP"`.

---

### Task 3: Acceptance — banked_ptr.c e2e + gpasm oracle

**Files:**
- Create: `crates/driver/tests/fixtures/banked_ptr.c`, `crates/driver/tests/banked_ptr_e2e.rs`
- Create: `crates/asm/tests/fixtures/banked_ptr.asm`, `crates/asm/tests/gpasm_banked_ptr.rs`

**Interfaces:**
- Consumes: Tasks 1–2 + `pic14_sim` + gpasm. Debug in the responsible stage; keep stage tests green.

- [ ] **Step 1: Write the failing acceptance program** — banked_ptr.c with arrays pushed into banks 1–3 and a hand-computable `out`:

```c
volatile unsigned char filler[80];   // 0x20-0x6F: fills bank 0 (region_for)
volatile unsigned char arrB1[16];    // 0xA0-0xAF (bank 1)
volatile unsigned char arrB2[16];    // 0x120-0x12F (bank 2)
volatile unsigned char arrB3[16];    // 0x1A0-0x1AF (bank 3)
volatile unsigned char out;

struct P { unsigned char a; unsigned char b; };
__attribute__((noinline)) struct P mk(void) {     // sret into a banked target
    struct P r; r.a = 5; r.b = 6; return r;
}

void main(void) {
    unsigned char i = 3;
    arrB1[i] = 0x11; arrB2[i] = 0x22; arrB3[i] = 0x33;   // FSR+IRP writes (3 banks)
    out = arrB1[i] + arrB2[i] + arrB3[i];                // 0x66 — FSR+IRP reads
    arrB2[5] = arrB1[1];                                 // banked direct copy (BANKSEL)
    arrB1[1] = 0x07;
    out = (unsigned char)(out + arrB2[5]);               // 0x66 + 0x07 = 0x6D
    struct P g;                                          // alloca; sret target
    g = mk();                                            // sret call into the frame
    out = (unsigned char)(out + g.a + g.b);              // 0x6D + 5 + 6 = 0x78
    arrB3[arrB2[2]] = 0x40;                              // chained dynamic index
    out = (unsigned char)(out + arrB3[0]);               // 0x78 + 0x40 = 0xB8
}
```

(Expected `out == 0xB8`. **Verify by hand during the task** — if clang -O1 folds/rewrites any piece (e.g. constant-folded indices, strength-reduced arithmetic), adjust the C to keep the SAME coverage (FSR+IRP writes+reads across all three banks, a banked direct copy, an sret call into a frame alloca, a chained dynamic index) and recompute; document the exact emitted IR + final value in the test. The `mk` struct local and `g` become allocas in main's frame — the sret target alloca must be ≤ 0x1EF with its span inside one window (small structs are fine). Keep all FSR bases + spans inside their windows — the loud asserts fire otherwise.)
- [ ] **Step 2: Write the acceptance test** — `banked_ptr_e2e.rs`: run the driver, simulate, assert `out` and `halted()`. Debug in the responsible stage.
- [ ] **Step 3: Write the gpasm cross-check** — fixture the driver's `.asm`, assert our HEX == gpasm byte-for-byte + same sim `out` (M6/M7/M8 pattern).
- [ ] **Step 4: Run the full suite** — all green (probe, overlay, banked, ptr_probe, array, scalar, structs, muldiv, banked_ptr).
- [ ] **Step 5: Commit** — `git commit -m "test(e2e): multi-bank FSR arrays compile and run correctly"`.

---

## Self-review notes

- **Spec coverage:** M9 removes the last bank-0-only constraint on indirect access — pointers/structs/arrays work across all 4 banks. Remaining deferred: const-table PCLATH page-crossing (the next big one — a 300+ byte RETLW table + the code-page model), `Global.addr` u16, dynamic-length memcpy, const structs, banked runtime routines.
- **Correctness risks (verify by SIMULATION):** (1) the IRP must be re-set on EVERY FSR setup (a bank-2 access followed by a bank-0 access without the BCF would silently hit bank 2 — the interleaved-bank SIM test pins this); (2) the FSR `& 0xFF` literal for bank-2/3 bases (`0x120 → 0x20`); (3) the window-span assert must use the OBJECT size (terms are runtime, bounded by span − 1); (4) the sret indirect IRP from the stored hi byte. Every one has a sim test.
- **The M5 "byte-identical" property changes deliberately:** every FSR setup now emits `BCF/BSF STATUS, 7` (needed for correctness once IRP can be left at 1). The fixtures/tests are updated, and the gpasm byte-identical cross-checks still hold (same new asm on both sides).
- **STATUS bit 7 vs direct banking:** setting IRP does not disturb the banking pass (sim's `bank_base` masks `STATUS<6:5>`); direct and indirect paths are independent. Verified against the sim's model; the acceptance exercises both interleaved.
- **Contract:** the window helper + the span lookup are the only new isel internals; no IR changes. The FSR-reachable GPR windows are the four alloc bank regions — arrays must fit within one.
