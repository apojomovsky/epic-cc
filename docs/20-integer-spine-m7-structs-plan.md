# Integer Spine — Milestone 7: Structs (phase-3 completion) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Structs compile — the phase-3 core (pointers, arrays, structs) completes. The surface: struct globals (`%struct.S = type { ... }`), `alloca` for struct locals, struct copy (`llvm.memcpy`), byval/sret call ABI, dynamic array-in-struct indexing (multi-index GEP chains), nested structs, byte-offset GEP forms. Acceptance: a program using structs the way embedded C does compiles, simulates correctly, and our HEX matches gpasm byte-for-byte.

**Architecture:** `crates/ir` + `crates/irparse` gain named-struct types (size/layout), `alloca`, `memcpy`, and a reworked GEP (`base` = global | reg; offset = constant byte `k` + scaled dynamic terms `Σ s×%reg`); `crates/alloc` sizes the new slots; `crates/isel` resolves GEP chains, lowers alloca/memcpy, and implements the byval/sret call ABI. All verification via `pic14-sim` + gpasm oracle, exactly like M5/M6.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

**Spec:** phase 3 of `docs/12-backend-design.md` — the phase-3 goal is "pointers, arrays, structs". M5 did pointers+arrays+const; this milestone is structs. **No spike reference exists for structs** — the lowering recipes below are derived from the clang probes in `/tmp/m7probe/` (s1–s9) and the PIC16 idioms already proven in M2–M6; every recipe is sim-verified.

## Global Constraints

- Build/test with `make exec CMD="cargo ..." …`; never bare `cargo`.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only.
- Text boundaries: `crates/ir` defines the canonical IR text format; `alloc` map and `.asm` are text artifacts. **The GEP canonical text changes this milestone** (see Task 1) — the M5 GEP round-trip/isel tests that assert the old `%p = gep @ram %3` text are updated deliberately, not worked around.
- Unsupported constructs panic loudly, never silently miscompile.

## The lowering recipes (load-bearing design)

### GEP model (from the s1/s8/s9 probes)

clang -O1 emits GEPs in these shapes (all must parse):
1. **Byte-offset form (constant field offsets)**: `getelementptr inbounds nuw (i8, ptr @g, i16 2)` → element type `i8`, index 2 → **offset bytes = 2** (stride 1). `(i16, ptr @x, i16 k)` → stride 2.
2. **Multi-index form (dynamic array index)**: `getelementptr inbounds nuw [4 x i8], ptr <base>, i16 0, i16 %2` → index 0 × aggregate stride (4) folds into the byte constant; the last index × element stride (1) becomes a scaled term. `[2 x i16], ptr @x, i16 0, i16 %i` → term `2×%i`.
3. **Chained base**: the outer GEP's base can be `%reg` — the result of another GEP (`ptr getelementptr inbounds nuw (i8, ptr @a, i16 1)`), a byval/sret param (`ptr %0`), or an alloca (`ptr %1`).

**Canonical IR:** `Gep { dst, base: GepBase, k: u8, terms: Vec<(u8 scale, String reg)> }` with `enum GepBase { Global(String), Reg(String) }`; text: `%d = gep <@g|%r> +<k> [+<s>*<%r>]...` (e.g. `%p = gep @a +1 +1*%2`). Chains compose: `gep_for(%outer)` = (base, k_inner + k_outer, terms_inner + terms_outer) — resolved eagerly at isel collection time (fixpoint scan; cycle/missing → panic).

**Inlined GEP operands:** `store volatile i16 5, ptr getelementptr inbounds nuw (i8, ptr @g, i16 2)` — clang inlines constant-offset GEPs into `load`/`store`/`call`/`memcpy` operands. irparse **materializes a synthetic Gep instruction** with a fresh reg name in the current block before the operand's instruction, then rewrites the operand to `%name`. (GEPs are virtual — no slot — so synthesis is free.)

**Attr-stripping hazard:** `strip_attrs` drops ALL tokens inside parens (its `range(...)`/`initializes(...)` skip). The paren GEP `(i8, ptr @g, i16 2)` would be destroyed by it. **GEPs must be parsed from the raw line** (its own `inbounds`/`nuw`/`nusw`/`inrange` attr stripping done inside the GEP parser), never via the generic strip.

### Pointer resolution in isel

`geps` map: `{func}::reg -> (Base, k, terms)` with `enum Base { Global(String), Slot(String /*name*/, bool /*indirect*/) }`.
- `Global(g)` — RAM: addr = global_addr(g); const (flash): RETLW path (i8 only; store-through-const still panics).
- `Slot(name, false)` — the base is a local slot (byval param copy, alloca, or any local): addr = slot_addr(cur_func, name).
- `Slot(name, true)` — **indirect** (sret param): the slot holds the target address; FSR = `[slot] + k + Σ terms`.
- Params seeded at collection: byval param `%0` → `Slot("0", false)` (the param slot IS the struct copy); sret param `%0` → `Slot("0", true)`; alloca `%1` → `Slot("1", false)` (the alloca's own slot).

Address emission (shared helpers `emit_ptr_load_byte(ptr: &Val, byte_off: u8)` / `emit_ptr_store_byte(ptr, byte_off, val)` used by Load, Store, AND Memcpy):
- **Direct base + constant offset** (no terms): plain `MOVF addr+k+off, W` / `MOVWF` — no FSR.
- **Direct base + terms** (dynamic): `FSR = base_addr + k + Σ s×%r` then INDF. Single-term fast path keeps the M5 shape (`MOVF %r,W; ADDLW base_lo; MOVWF FSR`); general sums accumulate in the existing `scratch` byte: `MOVLW 0; MOVWF scratch; [MOVF %r,W; ADDWF scratch,W; MOVWF scratch] per term (×2 = repeat ADDWF); MOVF scratch,W; ADDLW base_lo+k; MOVWF FSR`.
- **Indirect base** (sret): `FSR = [slot_lo] + k + Σ s×%r` — `MOVF slot_lo,W; ADDLW k; MOVWF FSR` (const) or the scratch-sum with the slot's contents as the base.
- FSR-reachable RAM is the low 256 bytes (bank-0 only — IRP follow-up). **Assert every FSR base (global, slot, or stored sret address) ≤ 0xFF, loudly.** The acceptance keeps FSR bases in bank 0.

### memcpy

`llvm.memcpy.p0.p0.i16(ptr d, ptr s, i16 N, i1 volatile)` → `Inst::Memcpy { dst: Val, src: Val, len: u8 }`. Lowering: for `i in 0..N`: `emit_ptr_load_byte(src, i)` → `MOVWF <dst byte addr>`. N must be a constant ≤ 255; volatile=true panics loudly. (Struct assignment in C → memcpy with the struct size, e.g. `i16 4` for `{i8, i16}`.)

### byval / sret call ABI

- **Callee byval** `f(ptr ... byval(%struct.S) align 2 %0)`: the param slot is a `size`-byte struct copy. Loads/stores through `%0`/GEPs of `%0` = direct `Slot("0", false)` access.
- **Callee sret** `make(ptr ... sret(%struct.S) align 2 %0)`: the param slot is 2 bytes holding the target address; loads/stores through `%0` = `Slot("0", true)` (FSR-indirect).
- **Caller byval arg**: copy `size` bytes from the arg's pointer (global / alloca / GEP reg) into the callee's param slot via `emit_ptr_load_byte(arg, i)`.
- **Caller sret arg**: store the target address into the callee's sret param slot: `MOVLW LOW(addr); MOVWF pa; MOVLW HIGH(addr); MOVWF pa+1`, with `assert!(addr <= 0xFF)` (bank-0 FSR reachability) — loud, documented. The target is a global or an alloca slot.
- Scalar args and the retval copy are unchanged.

### Struct layout (irparse)

`%struct.X = type { <field>, ... }` declarations → a type table. Field sizes: `i8`=1 (align 1), `i16`=2 (align 2), `[N x T]` = N × size(T) (align = align(T)), `%struct.Y` = recursive (align from its fields). Layout: each field at `round_up(off, align(field))`; struct align = max field align; **size = round_up(last end, struct align)** (so `{i8, i16}` → 4, matching clang's `i16 4` memcpy; `{i16, i8}` → 4). Sizes feed: struct global `@g = global %struct.S zeroinitializer, align 2` (size = type size, bytes = zeros), `alloca %struct.S, align 2` (`Inst::Alloca { dst, size }`), `byval(%struct.S)` / `sret(%struct.S)` param widths. Alignment does NOT affect placement (alloc's `min(size, 2)` global rule and byte-stepped locals already match M5/M6 semantics; struct fields are never accessed except through clang-folded byte offsets).

---

### Task 1: `ir` + `irparse` — types, alloca, memcpy, GEP rework, param attrs

**Files:**
- Modify: `crates/ir/src/lib.rs`, `crates/irparse/src/lib.rs`
- Test: `crates/ir/tests/roundtrip.rs`, `crates/irparse/tests/parse_ll.rs` (extend)

**Interfaces:**
- Produces: the new canonical IR (below). Consumers (alloc/isel) are updated in Tasks 2–4; until then the workspace may not compile — **keep `cargo build` green by landing the ir/irparse changes with their own tests and accepting that alloc/isel breakage is fixed in Tasks 2–4** (or stage: land ir text + parse first, then wire alloc/isel). Prefer landing Task 1 with irparse→isel contract tests green (parse_ll/roundtrip only; isel/alloc compile via minimal stub updates if needed — document exactly what you stubbed).

**Canonical text (new):**
- `%d = gep <@g|%r> +<k> [+<s>*<%r>]...`
- `%d = alloca <size>` (size decimal)
- `memcpy <dst> <src> <len>` (dst/src as `@g`/`%r`, len decimal; no dst reg — memcpy defines nothing)
- Params serialize inside the `fn` header as `<name>` plus byval size/sret flags: `fn <name>(<ret>) (<p1>|<p1>=byval<N>|<p1>=sret) ...`

- [ ] **Step 1: Extend the failing tests** — roundtrip: the new gep/alloca/memcpy texts + param forms; parse_ll: a `.ll` with `%struct.S = type { i8, i16 }`, `@g = global %struct.S zeroinitializer, align 2`, `alloca %struct.S, align 2`, `call void @llvm.lifetime.start.p0(...)`, `call void @llvm.memcpy.p0.p0.i16(ptr align 2 @g1, ptr align 2 @g2, i16 4, i1 false)`, the paren GEP `getelementptr inbounds nuw (i8, ptr @g, i16 2)`, the multi-index GEP `getelementptr inbounds nuw [4 x i8], ptr %1, i16 0, i16 %2` (reg base), the chained inlined GEP `store i8 1, ptr getelementptr inbounds nuw (i8, ptr %0, i16 2)`, and a `byval(%struct.S)`/`sret(%struct.S)` param signature. Assert: struct size 4 for `{i8, i16}`; Gep fields (base/k/terms); the inlined GEP became a synthetic reg + a preceding Gep inst; lifetime lines vanished; memcpy fields.
- [ ] **Step 2: Run to verify they fail** (unknown opcodes / wrong Gep fields).
- [ ] **Step 3: Implement** — ir structs (`Gep`, `GepBase`, `Alloca`, `Memcpy`, `Param { name, width, byval: Option<u8>, sret: bool }`, `CallArg { ty: Option<Ty>, val: Val, byval: Option<u8>, sret: bool }`, `Call.args: Vec<CallArg>`) + serialize; irparse: type table + layout, struct globals, alloca, memcpy (const len), lifetime strip (returns nothing), the new GEP parser (paren + multi-index + reg base + attrs, shared `parse_gep_expr` helper), inlined-GEP synthesis (`parse_inst` returns `Vec<Inst>`), param parsing (`ptr` + byval/sret attrs + `dead_on_unwind`/`noalias`/`nocapture`/`writable`/`writeonly`/`readonly`/`nonnull`/`initializes(...)` stripping), `zeroext`/`signext` returns (already stripped). GEPs parse from the RAW line (pre-strip_attrs).
- [ ] **Step 4: Run to verify they pass** — roundtrip + parse_ll green; document exactly which alloc/isel call sites are stubbed (if any) for the build.
- [ ] **Step 5: Commit** — `git commit -m "feat(ir,irparse): struct types, alloca, memcpy and reworked gep"`.

---

### Task 2: `alloc` — size the new slots

**Files:**
- Modify: `crates/alloc/src/lib.rs`
- Test: `crates/alloc/tests/alloc.rs` (extend)

**Interfaces:**
- Produces: Alloca → `(dst, size)` in `def_width`; Memcpy → `None`; params sized by `Param.width` (byval size | 2 for sret | scalar bytes). Struct globals already carry `Global.size` from irparse.

- [ ] **Step 1: Extend the failing test** — a module with `alloca` (4 bytes) + a byval param (4 bytes) + an sret param (2 bytes): assert the locals layout (the alloca reg and byval/sret params get their full widths, no overlap, ordering params-first).
- [ ] **Step 2: Run to verify it fails**.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run to verify it passes** — alloc tests + workspace compiles end-to-end again (isel still panics at runtime on the new insts — that's Task 3).
- [ ] **Step 5: Commit** — `git commit -m "feat(alloc): size alloca, byval and sret slots"`.

---

### Task 3: `isel` — pointer machinery (bases, chains, FSR sums, indirect, memcpy, alloca)

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: the `geps` map with `Base` (Global/Slot direct/indirect), chain folding (fixpoint; cycle panic), params/alloca seeded; `emit_ptr_load_byte`/`emit_ptr_store_byte` (direct, FSR-single-term fast path, FSR-scratch-sum, indirect) replacing the ad-hoc Load/Store pointer code; FSR-base ≤ 0xFF asserts; `Inst::Memcpy` (byte loop); `Inst::Alloca` → no-op (virtual like Gep); `Inst::Gep` → no-op (unchanged). Load/Store arms use the helpers.

- [ ] **Step 1: Extend the failing test** — assert emitted asm for: (a) direct byte-offset access `load i8 %p` where `%p = gep @g +2` → `MOVF <g+2>, W` (no FSR); (b) FSR sum `%p = gep @a +1 +1*%i` → the fast-path `MOVF %i,W; ADDLW <a+1>; MOVWF FSR`; (c) a 2-term sum `%p = gep @a +1 +2*%i` → scratch accumulation; (d) indirect (sret) `store i8 %v %p` with `%p` based on a `Slot(_, true)` → FSR from the slot contents; (e) memcpy `memcpy @g1 @g2 4` → 4 MOVF/MOVWF byte pairs; (f) an alloca-based base. PLUS a **simulation test**: assemble and run a module using the chain `%q = gep %p +1 +1*%2` (base = another GEP) against in/out globals, asserting the result (the s8 pattern: `@a + 1 + n`).
- [ ] **Step 2: Run to verify they fail** (panic: no gep / unsupported).
- [ ] **Step 3: Implement** per the recipes.
- [ ] **Step 4: Run to verify they pass** — isel tests + workspace (M5/M6 pointer tests must stay green — the single-term fast path must emit the identical FSR sequence).
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): pointer bases, chains, indirect and memcpy"`.

---

### Task 4: `isel` — byval/sret call ABI

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Produces: byval arg → `size`-byte copy into the callee's param slot; sret arg → the target address stored into the callee's sret param slot (`MOVLW LOW; MOVWF pa; MOVLW HIGH; MOVWF pa+1`, assert target ≤ 0xFF); callee-side param seeding already done (Task 3); scalar args + retval copy unchanged.

- [ ] **Step 1: Extend the failing test** — assert: (a) a byval call's emitted copy (`ptr_load_byte` × size into the param slot) + `CALL` + retval copy; (b) an sret call's address store into the sret param slot. PLUS **simulation tests** (the load-bearing ones): (i) `sum(struct Pair)` byval callee — caller builds a Pair in an alloca, calls, asserts the sum; (ii) `make()` sret callee — caller passes its alloca as the sret target, then reads the struct fields, asserting both bytes; (iii) a byval call with a GLOBAL arg (s6 pattern: `f(g)`).
- [ ] **Step 2: Run to verify they fail**.
- [ ] **Step 3: Implement** per the recipes.
- [ ] **Step 4: Run to verify they pass** — isel + workspace (existing call tests green — scalar path byte-identical).
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): byval and sret call abi"`.

---

### Task 5: Acceptance — structs.c e2e + gpasm oracle

**Files:**
- Create: `crates/driver/tests/fixtures/structs.c`, `crates/driver/tests/structs_e2e.rs`
- Create: `crates/asm/tests/fixtures/structs.asm`, `crates/asm/tests/gpasm_structs.rs`

**Interfaces:**
- Consumes: Tasks 1–4 + `pic14_sim` + gpasm. Debug in the responsible stage; keep stage tests green.

- [ ] **Step 1: Write the failing acceptance program** — structs.c exercising the whole M7 surface with a hand-computable `out`:

```c
struct Pair  { unsigned char a; unsigned short b; };
struct A     { unsigned char n; unsigned char v[4]; };
struct Outer { struct Pair in; unsigned char z; };

volatile unsigned char out;
volatile struct Pair g;
volatile struct A    arr;

__attribute__((noinline)) unsigned char sum(struct Pair p) {      // byval
    return (unsigned char)(p.a + p.b);
}
__attribute__((noinline)) unsigned char pick(struct A x) {        // byval + dynamic array-in-struct
    return x.v[x.n];
}
__attribute__((noinline)) struct Pair mk(unsigned char a, unsigned short b) {  // sret
    struct Pair r; r.a = a; r.b = b; return r;
}
void main(void) {
    g = mk(3, 0x1234);                    // sret call + struct copy (memcpy)
    out = sum(g);                         // byval from a global: 3 + 0x34 = 0x37
    arr.n = 2; arr.v[2] = 0x5A; arr.v[arr.n] = 0x11;             // dynamic struct-array store
    out = (unsigned char)(out + pick(arr));                     // 0x37 + 0x11 = 0x48
    struct Outer o; o.in.a = 1; o.in.b = 2; o.z = 3;            // nested structs (folded byte GEPs)
    out = (unsigned char)(out + o.in.a + o.in.b + o.z);         // 0x48 + 1 + 2 + 3 = 0x4E
}
```

(Expected: `out == 0x4E` for the fixed inputs. **Verify by hand during the task** — if clang folds/rewrites any expression out of the supported surface (e.g. `arr.v[arr.n] = 0x11` becomes a memcpy or a different chain), adjust the C to keep the SAME semantic coverage and recompute; document the exact emitted IR and the final expected value in the test.) `mk`'s struct local becomes an alloca; `o`/`r` locals become allocas or are SROA'd — either is fine. FSR targets must stay ≤ 0xFF: keep the program small (bank-0 frames); assert loudly otherwise and trim if needed.
- [ ] **Step 2: Write the acceptance test** — `structs_e2e.rs`: run the driver, simulate, assert `out == 0x4E` and `halted()`. Debug in the responsible stage (irparse → alloc → isel).
- [ ] **Step 3: Write the gpasm cross-check** — fixture the driver's `.asm`, assert our HEX == gpasm byte-for-byte + same sim `out` (M5/M6 pattern).
- [ ] **Step 4: Run the full suite** — all green (probe, overlay, banked, ptr_probe, array, scalar, structs).
- [ ] **Step 5: Commit** — `git commit -m "test(e2e): structs compile and run correctly"`.

---

## Self-review notes

- **Spec coverage:** M7 completes phase 3 — pointers, arrays, structs. The remaining big surface (mul/div/shl/shr runtime, multi-bank FSR+IRP, const-table PCLATH) is deferred to later milestones.
- **Correctness risks (verify by SIMULATION, not just emitted text):** (1) the multi-index GEP folding (const prefix indices fold into `k` with the aggregate stride — the current parser's silent drop of nonzero const prefixes is a latent bug this milestone fixes); (2) FSR sums with multiple scaled terms; (3) the sret indirect path (slot holds an address; FSR = contents + offset); (4) byval/sret caller copies. Every one of these must have a sim test.
- **The strip_attrs paren hazard is real:** the paren GEP would be destroyed by the generic attr stripper — the GEP parser must run on the raw line.
- **Deferred (later milestones):** mul/div/shl/shr (runtime library); FSR+IRP multi-bank arrays; const-table page-crossing (PCLATH); `Global.addr` u16; dynamic-length memcpy (loop); const structs (`constant %struct.S`) — panics loudly today.
- **Contract:** `GepBase`/`k`/`terms` and `Param.width`/`byval`/`sret` are the cross-crate contracts; the canonical GEP text change is deliberate (M5 tests updated).
