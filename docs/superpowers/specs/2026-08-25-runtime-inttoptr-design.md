# Runtime-address `inttoptr` lowering: FSR/INDF via slot-indirect

**Date:** 2026-08-25
**Parent:** `docs/31-ecosystem-integration-design.md` (HAL-2), epic-hal#67 item 2
**Ticket:** `epic-cc#117`
**Status:** design, awaiting approval

---

## Problem

epic-cc handles `inttoptr` only when the operand is a compile-time constant
(the literal-pointer `"0x<K>"` form). A runtime address value cannot be
dereferenced, so epic-hal keeps duplicated literal if/else chains in
`pic16_irq.c` (`EPIC_IRQ_ClearFlag` / `EPIC_IRQ_GetFlag`) behind
`#ifndef EPIC_AT`: `pir_reg_addr(d)` computes `PIR1` (0x0C) vs `PIR2` (0x0D)
at runtime from a const table field, and both implementations must stay in
step in a file where drift is an interrupt bug.

Pinned clang 20.1.8 (`-target msp430`, the driver's `-O1`, which is also the
epic-cc CLI epic-hal drives) produces **three shapes** for runtime SFR
addresses, verified against the real `pic16_irq.c` table and function
shapes:

1. **Standalone runtime `inttoptr`** (`read_offset`-style, table-free):

   ```llvm
   %4 = zext nneg i8 %3 to i16
   %5 = inttoptr i16 %4 to ptr
   %6 = load volatile i8, ptr %5, align 1
   ```

2. **Pointer-typed select of literal inttoptrs** (the dominant HAL shape,
   `EPIC_IRQ_ClearFlag` / `EPIC_IRQ_GetFlag`):

   ```llvm
   %18 = select i1 %17, ptr inttoptr (i16 12 to ptr), ptr inttoptr (i16 13 to ptr)
   %19 = load volatile i8, ptr %18, align 1
   ```

3. **Pointer-typed phi mixing a select result and a literal** (the
   `EPIC_IRQ_GetFlag` -O1 shape: INTCON path joins the PIR1/PIR2 select
   path):

   ```llvm
   %14 = select i1 %13, ptr inttoptr (i16 12 to ptr), ptr inttoptr (i16 13 to ptr)
   %16 = phi ptr [ %14, %10 ], [ inttoptr (i16 11 to ptr), %3 ]
   %17 = load volatile i8, ptr %16, align 1
   ```

Today all three fail loudly: shape 1 parses as a generic cast (`Inst::Zext`,
i16 -> i16, indistinguishable from a real zext) and its result has no
`resolve_pointers` entry, so isel panics `no gep for pointer %5`; shapes 2
and 3 have no `select`/`phi` fold path for literal arms (`iselcore` panics
`cyclic or unresolvable pointer chain`, `no gep for pointer`).

The requirement is not a generic pointer ABI. It is: **a pointer VALUE whose
bytes are a runtime address can be dereferenced as a volatile SFR**, on PIC14
via `FSR`/`INDF` and on PIC18 via `FSR0`/`INDF0` (ADR-009's existing pointer
model), with one access per source access and no reordering.

## Design: the address is a slot value, the deref is slot-indirect

The fold map already has exactly the representation we need: `Base::Slot(name,
true)` means "the slot at `name` holds a target ADDRESS, not the object
itself", and both backends already lower every deref of it through the
indirect FSR machinery (`emit_fsr_indirect` on PIC14, `emit_fsr0_indirect_slot`
on PIC18), reading the low/high address bytes from the slot, setting
IRP/bit-8 (PIC14) or `FSR0L`/`FSR0H` (PIC18), and accessing through
`INDF`/`INDF0`. That path is what `sret` params use today and it reaches any
data address, including SFRs, with no compile-time bank knowledge.

So the design is: **make a runtime-address pointer value materialize its
two address bytes into a slot, seed that slot as `Base::Slot(dst, true)`,
and let the existing indirect deref do the rest.** No new dereference code
in either backend.

### 1. A dedicated `IntToPtr` instruction

irparse currently folds `inttoptr` and `zext` into one `Inst::Zext` (both
parse to `to: I16`), which is exactly the kind of silent shape confusions
this pipeline panics on. Add `Inst::IntToPtr` (`dst`, `from: Ty` (I16), `val:
Val`, `to: Ty` (I16)):

- **irparse**: `%x = inttoptr <ty> <val> to ptr` -> `Inst::IntToPtr`; the
  generic cast branch keeps `zext`/`sext`/`trunc`/`ptrtoint` handling
  unchanged.
- **ir**: serialize/deserialize the new inst (`%d = inttoptr <from> <val> to
  ptr`), and size it in the allocator's value sizing (`to.bytes()` = 2).
- **isel (PIC14)** and **isel-pic18**: lower as a 2-byte value copy from the
  source's slot (or MOVLW for a `Const`), exactly like the existing `Zext`
  i16->i16 shape. The address bytes land in the dst slot.
- `ptrtoint` stays out of scope (today: `Inst::Trunc`, loud panic on the
  HAL's shapes; clang does not emit it at -O1 for these functions).

### 2. Seed runtime pointer slots in `iselcore::resolve_pointers`

Seeds, keyed `{func}::{reg}` like every other entry, all `(Base::Slot(dst,
true), 0, [])` (no constant offset, no terms):

- every `Inst::IntToPtr` dst;
- every pointer-typed `Inst::Select` whose two arms are both `Val::Const`
  (the literal inttoptr arms);
- pointer-typed `Inst::Phi` whose every incoming value is a `Val::Const` or a
  register already seeded as a runtime slot (fixpoint: inttoptr/select seeds
  land in one pass, phi seeds iterate). A phi with any incoming that is a
  folded (compile-time) pointer keeps the existing loud panic: its arm's
  bytes do not live in a slot, and materializing them would be new code for a
  shape clang does not emit here.

The seeds are ordinary fold-map entries: a GEP on top of a runtime pointer
(`(d)->pir_is_pir2` field access is exactly that) folds `(Base::Slot(dst,
true), k, terms)` and dereferences through the same indirect path with the
offset added in the FSR computation. The three shapes above all converge on
one mechanism.

### 3. The select/phi materialize address bytes as values

- **isel `Inst::Select`, ptr arm**: today a pointer select emits nothing
  (fold-only). A select whose arms are both constants now materializes its
  address bytes: emit it as an **i16 value select** (`emit_select(dst, cond,
  I16, a, b)`) which writes the two bytes of the chosen address into the dst
  slot, matching the seeded fold. Pointer selects that folded to a
  compile-time base keep emitting nothing. The discriminator is the arms
  being `Val::Const` (the same condition the seed uses), not a map lookup.
- **isel `phi` elimination** is value-based already: the phi copies move the
  incoming's bytes into the dst slot per edge (`emit_phi_copies`). A `Const`
  incoming copies its two bytes via `MOVLW`; a runtime-slot reg copies its
  two bytes. The seeded `(Base::Slot(dst, true))` then dereferences
  correctly. No phi-specific emission code is needed.

### 4. Volatile semantics and banking (stated, per the issue)

- **One access per source access**: each `load volatile` / `store volatile`
  through a runtime pointer emits exactly one FSR/`INDF` setup + access per
  byte. FSR is re-set per byte from the slot (existing indirect machinery
  has no auto-increment), so no two source accesses share state and nothing
  can be reordered or eliminated by the backend; the value slot is a plain
  local the allocator places, and the `and`/`xor` between an RMW load and
  store never clobbers it.
- **Banking**: the indirect access itself never needs `BANKSEL`. On PIC14
  `INDF`'s effective address is FSR + IRP directly over the whole linear
  file space (SFRs included), so no bank register is consulted; the design
  decision is stated here and will be recorded in the backend docstring:
  **a runtime SFR address is reached without any static BANKSEL; the stored
  high byte's bit 8 sets IRP (PIC14) or `FSR0H` carries it (PIC18)**. The
  slot holding the address bytes is an ordinary banked GPR; its own direct
  accesses (MOVF slot, BTFSS slot+1) go through the banking pass like any
  other local, which is already correct because the banking pass sees the
  slot's static address.
- **PIC18**: `Base::Slot(_, true)` already derefs via `FSR0L/FSR0H` +
  `INDF0` (`emit_fsr0_indirect_slot`), with no BSR involvement (access-bank
  SFRAM reachable, ADR-009). Only the new `IntToPtr` lowering and the select
  materialization are additive there.

## Scope

1. `ir` + `irparse`: `Inst::IntToPtr` (parse, serialize, round-trip, alloc
   sizing). **fails**: irparse round-trip test for shape 1.
2. `iselcore`: the three runtime-slot seeds above (inttoptr, const-arm
   select, fixpoint phi).
3. `isel` (PIC14): `IntToPtr` 2-byte copy; pointer-select-with-const-arms
   emits the i16 value select; deref unchanged; banking docstring for the
   indirect-SFR rule.
4. `isel-pic18`: the same two lowerings; deref unchanged (ADR-009).
5. Fixtures (below) + unit tests for each shape at the isel level.

## Acceptance

A driver e2e fixture (`crates/driver/tests/fixtures/runtime_sfr.c` +
`runtime_sfr_e2e.rs`) replicating the real `pic16_irq.c` shapes:

- a const `irq_desc_t` table, `pir_reg_addr(d)` computed at runtime, the
  address bytes flowing through **all three shapes** (inttoptr, const-arms
  select, select+phi join);
- `EPIC_IRQ_GetFlag` / `EPIC_IRQ_ClearFlag` equivalents with volatile reads
  and an RMW, `irq` coming from a global the sim preloads so both PIR1 and
  PIR2 arms are exercised;
- committed HEX + a gpasm cross-check (the fixture shape the other codegen
  fixtures use);
- the simulator observes the accesses at 0x0C / 0x0D for the two `irq`
  values (the PIC14 sim sees SFR loads/stores), and the flag/clear results
  in observables;
- `make test` green, `make check-warnings` clean.

Plus: isel unit tests pinning the emitted sequence for each of the three
shapes (direct `INDF` access, no BANKSEL, one setup per byte), and an
isel-pic18 unit test for the same shapes through `INDF0`.

epic-hal deleting the `#ifndef EPIC_AT` chains is the acceptance of
epic-hal#67 item 2, not this ticket; this fixture proves the compiler side of
it.

## Risks

- **clang shape drift** (arm order, phi join, `zext` vs direct `inttoptr`).
  The fixture pins all three shapes in source; if clang changes which one it
  emits for a given source, the parse/seed handles all three, and the fixture
  still passes (the shapes are equivalent). If clang emits a NEW shape
  (pointer arithmetic `add` on a pointer, `ptrtoint` materialization), it
  stays a loud panic, never a miscompile.
- **Phi seeding fixpoint**: the cyclic-phi case (loop-carried pointer)
  panics loudly as today; only feed-forward phi joins are seeded.
- **`SFR address < 0x100` assumption on PIC14**: the indirect path handles
  any 9-bit address (IRP); 16-bit addresses via the slot high byte. No new
  restriction.
- **PIC18 coverage depth**: PIC18 e2e runs on the driver's PIC18 target if
  the sim/gpasm gate supports it; otherwise isel-pic18 unit tests plus the
  shared iselcore seeds are the gate. State explicitly at review.
