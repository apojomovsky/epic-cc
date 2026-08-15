# Phase 3 — Milestone 5: Pointers, Arrays, and Const-in-Flash Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** C pointers and arrays compile correctly: `getelementptr` lowers to address arithmetic, RAM accesses through a runtime pointer go through `FSR`/`INDF`, and `const` tables (the Harvard split) lower to `RETLW` lookup tables. The milestone acceptance: the pointer/const probe (runtime RAM pointer + `const` table read — the exact case the feasibility spike proved) compiles through the full pipeline and runs correctly (`out == 20` for `in == 1`, halted), plus a RAM-array probe, cross-checked against `gpasm`.

**Architecture:** `crates/ir` gains a `Gep` instruction (base global + offset) and array/`constant` globals with sizes and initializer bytes; `irparse` parses `getelementptr` and the array-global forms; `alloc` sizes arrays and gives `const` globals **no** RAM address (flash); `isel` lowers GEPs inline (recompute the address at each use, per the spike), `load`/`store` through a pointer via `FSR`/`INDF` for RAM, and `const` reads via `CALL` into a `RETLW` table. `pic14-sim` already models FSR/INDF/PCL/RETLW (phase 1). Structs (`sret`/`byval`) are the next milestone.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

**Spec:** [`docs/12-backend-design.md`](../12-backend-design.md) §4 phase 3; the proven reference is [`docs/11-pointer-const-findings.md`](../11-pointer-const-findings.md) and the spike (`spike/src/codegen.rs`, `spike/src/ir.rs`, on disk, gitignored).

## Global Constraints

- Build/test with `nix develop --command cargo …`; never `apt install` toolchain deps.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only; GPL never linked.
- Text boundaries: stages communicate via text; the `ir` crate defines the IR text format; the `alloc` map and `.asm` are text artifacts.
- New files must be `git add`ed before `nix develop` sees them.
- Unsupported constructs panic loudly, never silently miscompile.

## The pointer/const lowering (the load-bearing design — from the spike, docs/11)

- `getelementptr <ty>, ptr @base, <idx1>, <idx2>, …` reduces to `base + effective_offset` where the effective offset is the **last index** (the earlier indices are array/struct-selector zeros for the forms we accept). The `ir::Gep` carries `{ dst, base, offset: Val }` (offset is the runtime/constant last index).
- A GEP result is a **virtual pointer** (no RAM slot): each `load`/`store` through it recomputes the address.
- **RAM access** (`base` is a non-const global): `MOVF idx,W; ADDLW base_lo; MOVWF FSR; MOVF INDF,W` (load) / `MOVWF INDF` (store). The address is 8-bit within a bank; FSR + IRP cover all banks — the spike used bank 0 (IRP=0), and our `alloc` keeps arrays in the bank-0/common region for this milestone (a banked-array milestone follows).
- **Flash access** (`base` is a `const` global): `CALL __read_<base>` into `ADDLW LOW(<base>); MOVWF PCL` followed by the `RETLW k` table; the callee returns with the value in W. `PCLATH` stays 0 for our page-0 programs (the page-crossing caveat from docs/11 is recorded, not solved).
- `const` globals get no RAM address (the map has no entry); `isel` distinguishes them via `Module.globals[].is_const`.

## IR text format additions (`crates/ir`)

```
%d = gep @<base> <val>              ; gep @ram %3 — base + offset
global <name> <ty> @0xNN            ; RAM globals as today
const <name> <ty> @0xNN             ; const globals: NO addr (flash); <ty> may be an array
```

`Global` gains `size: u8` (total bytes) and `bytes: Vec<u8>` (initializer contents, used for `const` tables). Array globals: `global ram i8 @0x25` with `size: 8` (the `.ll` `[8 x i8]`), `const table i8` with `size: 4, bytes: [10,20,30,40]` (the `.ll` `constant [4 x i8] c"\0A\14\1E("`). The `size` is what `alloc` uses to advance addresses.

---

### Task 1: `ir` — Gep instruction and array/const globals

**Files:**
- Modify: `crates/ir/src/lib.rs`
- Test: `crates/ir/tests/roundtrip.rs` (extend)

**Interfaces:**
- Produces: `Inst::Gep { dst, base: String, offset: Val }` with canonical `%d = gep @<base> <val>`; `Global { name, is_const, ty, size: u8, bytes: Vec<u8>, addr: Option<u16> }` with canonical `global <name> <ty> [@0xNN]` / `const <name> <ty>` lines (const globals serialize without an address); parse accepts the size/bytes fields. `size` defaults from `ty.bytes()` when parsing a scalar global line.

- [ ] **Step 1: Extend the failing test** — round-trip a module with `%p = gep @ram %3`, a `const table i8` global (no addr), and `global ram i8 @0x25` (size 8): assert the round-trip fixed point and that the const line has no `@addr` and the gep line round-trips.
- [ ] **Step 2: Run to verify it fails** — `nix develop --command cargo test -p ir`.
- [ ] **Step 3: Implement** — the Gep variant + serialize/parse; Global.size/bytes + serialize/parse (the canonical `const <name> <ty>` line carries no addr; `global <name> <ty> [@0xNN]`; `size`/`bytes` are metadata — decide how to carry them in the canonical text: simplest is to extend the lines to `global <name> <ty> <size> @0xNN` — but that breaks the existing driver map parser; **alternative: keep the map text unchanged (`global <name> 0xNN`) and let the driver get `size`/`bytes` from the `ir` Module directly** (the driver has the Module). Ruling: `size` and `bytes` are carried in the `ir::Global` struct only (not the alloc map text); the alloc map text stays `global <name> 0xNN` / `const <name>` (const listed without addr, for isel to see it exists).
- [ ] **Step 4: Run to verify it passes**.
- [ ] **Step 5: Commit** — `git commit -m "feat(ir): gep instruction and sized globals"`.

---

### Task 2: `irparse` — getelementptr and array/const globals

**Files:**
- Modify: `crates/irparse/src/lib.rs`
- Test: `crates/irparse/tests/parse_ll.rs` (extend)

**Interfaces:**
- Produces: `parse_ll` handles `getelementptr` (port `spike/src/ir.rs`'s GEP parsing: split on `, ptr `, base from the first token, offset = last index), and the array-global forms: `@ram = global [8 x i8] zeroinitializer` (size 8, bytes zeros), `@table = constant [4 x i8] c"\0A\14\1E("` (size 4, bytes decoded — port `parse_string_literal` from the spike). `Global.size` = `N * elem_bytes`; `Global.bytes` = the init.

- [ ] **Step 1: Extend the failing test** — parse a `.ll` with `@ram = global [8 x i8] zeroinitializer`, `@table = constant [4 x i8] c"\0A\14\1E("`, and a function with `%p = getelementptr i8, ptr @ram, i16 %3` and `%q = getelementptr [4 x i8], ptr @table, i16 0, i16 %3`; assert sizes, bytes, and the Gep bases/offsets.
- [ ] **Step 2: Run to verify it fails**.
- [ ] **Step 3: Implement** — port the spike's GEP and global parsing, adapting to the struct-form `ir` and the canonical syntax.
- [ ] **Step 4: Run to verify it passes**.
- [ ] **Step 5: Commit** — `git commit -m "feat(irparse): parse getelementptr and array globals"`.

---

### Task 3: `alloc` — size arrays, skip const globals in RAM

**Files:**
- Modify: `crates/alloc/src/lib.rs`
- Test: `crates/alloc/tests/alloc.rs` (extend)

**Interfaces:**
- Produces: `allocate` sizes globals by `Global.size` (not `ty.bytes()`), assigns RAM addresses to non-const globals only (const globals get `addr: None`), and includes `const` globals in the map text as `const <name>` (no address) so `isel` sees them; the map text keeps `global <name> 0xNN` for RAM globals.

- [ ] **Step 1: Extend the failing test** — a module with `global ram i8` (size 8) and `const table i8` (size 4): assert `ram` gets an address, `table.addr == None`, and `map_text` contains `const table` and `global ram 0xNN`.
- [ ] **Step 2: Run to verify it fails** (alloc currently sizes by ty.bytes() and may not emit const lines).
- [ ] **Step 3: Implement** — use `size`, keep const globals unaddressed, emit `const` lines in the map.
- [ ] **Step 4: Run to verify it passes** — the existing banked/overlay tests still pass (their globals are non-const, sized 1).
- [ ] **Step 5: Commit** — `git commit -m "feat(alloc): size arrays and keep const globals out of RAM"`.

---

### Task 4: `isel` — GEP, FSR/INDF, and RETLW

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Consumes: `ir::Inst::Gep`, `Module.globals[].is_const/size/bytes`, the address map.
- Produces: the pointer/const lowering. Port from `spike/src/codegen.rs` (the verified reference):
  - GEPs tracked in a `geps: HashMap<String, (String, Val)>` (keyed `{func}::{dst}`), built in a pre-pass.
  - `load`/`store` with a `Ptr::Reg(r)` operand: look up the GEP; if `base` is a `const` global → `CALL __read_<base>` + `MOVWF dst` (load only; a store to const panics); else → FSR/INDF: `MOVF <offset>,W; ADDLW <base_lo>; MOVWF FSR; MOVF INDF,W; MOVWF dst` (load) / `MOVWF INDF` (store). The `offset` is a `Val` (Reg → its slot low byte; Const → `MOVLW`).
  - `const` tables emitted after the functions: `__read_<base>:` `ADDLW LOW(<base>)` `MOVWF PCL` `<base>:` `RETLW <b0>` `RETLW <b1>` … per `Global.bytes`.
  - The existing `Ptr::Global(name)` path is unchanged (direct RAM access).
- **Keep the i1/icmp/brcond/select, call/ret, and phi-copy behavior intact** — the probe/overlay/banked e2e must still pass.

- [ ] **Step 1: Extend the failing test** — a module with `@ram` (size 8) and `@table` (const, bytes [10,20,30,40]) and a function: `%i = load i8 @in`, `%p = gep @ram %i`, `%t = gep @table %i`, `%v = load i8 %t`, `store i8 %v %p`, `ret void`. Assert: the FSR/INDF sequence (`MOVWF FSR`, `MOVF INDF, W`) for the RAM store/load; `CALL __read_table` + `MOVWF` for the const load; and the emitted table (`__read_table:`, `ADDLW LOW(table)`, `MOVWF PCL`, `table:`, `RETLW 0x0A` … `RETLW 0x28`).
- [ ] **Step 2: Run to verify it fails** (isel panics on Gep / Ptr::Reg).
- [ ] **Step 3: Implement** — the spike port. `base_lo` comes from the map (RAM global address) or the table's own label (flash). Note: a `const` global's `base` in the FSR path is never used (const reads go to RETLW); the `geps` map distinguishes by `Module.globals[base].is_const`.
- [ ] **Step 4: Run to verify it passes** — isel tests + the full suite (probe/overlay/banked e2e unregressed).
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): pointers via FSR/INDF and const tables via RETLW"`.

---

### Task 5: Acceptance — the pointer/const probe and a RAM array

**Files:**
- Create: `crates/driver/tests/fixtures/ptr_probe.c`, `crates/driver/tests/ptr_probe_e2e.rs`
- Create: `crates/driver/tests/fixtures/array.c`, `crates/driver/tests/array_e2e.rs`
- Create: `crates/asm/tests/fixtures/ptr_probe.asm`, `crates/asm/tests/gpasm_ptr.rs`

**Interfaces:**
- Consumes: the full pipeline (Tasks 1–4) and `pic14_sim`.

- [ ] **Step 1: Write the failing pointer probe** — `ptr_probe.c` (the spike's proven probe, from docs/11):

```c
volatile unsigned char in;
volatile unsigned char out;
static const unsigned char table[4] = {10, 20, 30, 40};
volatile unsigned char ram[8];
void main(void) {
    unsigned char i = in & 3;
    volatile unsigned char *p = ram + i;
    *p = table[i];
    out = *p;
}
```

Expected: `in = 1` → `out = 20`, halted. `ptr_probe_e2e.rs` runs the driver, simulates, asserts `out == 20` and `halted()`; also asserts the `.asm` contains `CALL __read_table` and `MOVWF FSR` (both lowerings engaged).

- [ ] **Step 2: Write the RAM-array probe** — `array.c` (a non-const array written and read at a runtime index, no const involvement):

```c
volatile unsigned char in;
volatile unsigned char out;
volatile unsigned char buf[8];
void main(void) {
    unsigned char i = in & 7;
    buf[i] = (unsigned char)(i + 1);
    out = buf[i];
}
```

Expected: `in = 3` → `out = 4`, halted (buf[3] = 4). `array_e2e.rs` asserts it.
- [ ] **Step 3: Run to verify they fail, then make them pass** — debug in the responsible stage (likely isel GEP/FSR handling); keep stage tests green. If `-O1` folds the probe (e.g. `out = *p` becoming `out = table[i]`), add a `volatile` barrier or adjust until the `.ll` keeps the pointer (the spike's probe survived at `-O1` with volatile).
- [ ] **Step 4: Write the gpasm cross-check** — capture the driver's `.asm` for `ptr_probe.c`, fixture it, assert our HEX == gpasm byte-for-byte and the sim run gives `out == 20` (mirror the M3/M4 pattern).
- [ ] **Step 5: Run the full suite** — all green (probe, overlay, banked, ptr_probe, array e2e).
- [ ] **Step 6: Commit** — `git commit -m "test(e2e): pointers, arrays, and const tables run correctly"`.

---

## Self-review notes

- **Spec coverage:** milestone 5 implements the phase-3 core — the FSR/INDF pointer path, RAM arrays, and the Harvard const split (RETLW tables) — exactly the case the feasibility spike proved (docs/11). Structs (`sret`/`byval`) and banked arrays (FSR + IRP across banks) are the next milestones.
- **Deferred (later milestones, panic loudly until then):** structs/sret/byval; `const` tables crossing 256-word pages (PCLATH — recorded in docs/11); multi-bank arrays (IRP); GEP with pointer-to-pointer or non-global bases.
- **Correctness notes for the implementer:** the spike is the verified reference — port, don't redesign; keep the i1/icmp/brcond/select, call/ret, and phi-copy behavior intact (the existing e2e are the regression net). The `alloc` map text format stays `global <name> 0xNN` (the driver/isels parse it); `size`/`bytes` live only in the `ir::Global` struct.
- **Type consistency:** `ir::Inst::Gep`, `Global{size, bytes}`, `isel`'s `geps` map keyed `{func}::{dst}` — stable across tasks; the canonical IR line forms and the map text are the contracts.
