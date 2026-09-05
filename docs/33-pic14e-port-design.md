# 33: PIC14E (Enhanced Mid-range) port design

> **Approval status:** revision 2, addressing an independent review
> (2026-09-05) that held the previous version rather than approving it.
> The review found a correctness-class error in this document's own
> reasoning (the skip-window hazard does not disappear on this core, it
> survives; see D-1 and the table in section 1), an architectural
> question incorrectly deferred to P3 that actually gates P0 (the
> linear-region design, resolved in D-2 below), and a set of factual
> errors in the device-data sketch (D-3). All are addressed in this
> revision. **Still pending user approval**, and two external
> preconditions block starting P0 regardless of approval: DS41364B is
> not vendored in this repo (`vendor/` has no PIC16F193X datasheet), and
> whether `gputils` 1.5.2 knows `-p p16f1937` and ships its `.lkr` has
> not been checked. Both are named again in D-3 and section 4.
> This document is the design of record for
> [issue #228](https://github.com/apojomovsky/epic-cc/issues/228). The
> implementation plan derives from it and does not exist yet.

**Target:** the PIC16F1937 / PIC16F1939 family (the 1933/1934/1936/1938
share the core with less flash and RAM; the PIC12F1xxx parts share the
core once it exists, noted but not scoped here). The 1937/1939 are
popular hobbyist Enhanced Mid-range parts, and `pic16f193x-hal` is named
in epic-hal's own ecosystem-integration decisions
(`epic-cc/docs/31-ecosystem-integration-design.md` D-1) as an enhanced
mid-range HAL, currently **deferred** there specifically because
"epic-cc has no backend for that core." **This needs reconciling before
approval, not after:** confirm in epic-hal what state `pic16f193x-hal`
is actually in (a real, XC8-verified HAL that would serve as a register
reference and cross-check per section 6, or a placeholder deferred
pending exactly this port) before relying on it as this document's
stated reason for picking this family.

**Definition of done, rescoped from revision 1.** Revision 1 defined
parity against "what the PIC14 backend supports today," written when
the PIC14 backend had roughly fifteen e2e fixtures (the PIC18 port's
own phrasing, copied forward). It now has 61. Matching that in full is
not this port's v1 goal. **v1 parity is scoped to the PIC18-era
subset**: the P2-P7 phases below, matching the shape of capability the
PIC18 port itself reached before its own P4-P8 follow-up phases
extended it further. Explicitly out of scope for v1, each a named
follow-up: inline assembly (ADR-017), indirect calls and function
pointers including the `CALLW`-based lowering they would need
(ADR-022), cross-context callbacks (ADR-024), multi-TU via `llvm-link`
(ADR-011), `EPIC_AT`/`EPIC_CONFIG`/`EPIC_FOSC_HZ` (ADR-012, and
`crates/driver/src/fosc.rs:40` already has a `Core::Pic14e` panic
waiting for this), the size/map report (ADR-025), the address-to-line
table (ADR-028), and the `-fpack-struct` question ADR-026 settled for
PIC18 but this document never asks for PIC14E.

---

## 1. Why the ISA is a smaller job than the PIC18 port, and the allocator is not

The PIC18 port was a new core in almost every dimension: 16-bit words,
byte-oriented PC, two-word instructions, an access bank, hardware
multiply, `TBLRD`, two interrupt vectors. PIC14E keeps the PIC14
foundation and adds a layer on top, on the ISA side. Every figure in
this table is working knowledge and must be confirmed against the PIC16F193X
datasheet (DS41364B) before it is hard-coded, which is currently
impossible in this repo (see the approval-status note above); items
most worth checking first are flagged `[VERIFY]`, matching the
convention in [`01-target-pic14.md`](01-target-pic14.md).

| PIC14 constraint | PIC14E |
|---|---|
| 14-bit instruction words | Same 14-bit words; the byte-oriented, bit-oriented and literal opcode families are claimed to be **bit-identical encodings** to the classic set (DS41364B Table 26-3). `[VERIFY]`, unflagged in revision 1 despite being the load-bearing claim behind D-1. Also confirm that `OPTION`/`TRIS f` are removed and STATUS bits 5-7 (RP0/RP1/IRP) are unimplemented on this core, since the backend would otherwise silently emit dead classic-PIC14 bank-select code that assembles but does nothing; this deserves a P1 negative test, not an assumption. |
| 8-level hardware stack | 16-level. DS41364B adds `STKPTR`/`TOSL`/`TOSH` (readable/writable) and `STVREN` (stack over/underflow reset); still not addressable for compiler purposes in v1, and none of the new stack facilities are used. |
| Bank via RP1:RP0 bits in STATUS | Dedicated 5-bit `BSR` register, `MOVLB k` (DS41364B §2.2, Table 26-3) |
| 16 bytes of `BANKSEL`-free common RAM (0x70-0x7F) | Same 16 common bytes, reachable from any bank (DS41364B §2.2). Today's PIC14 backend already spends 14 of those 16 bytes on fixed scratch/retval/ISR-save (`crates/driver/src/report.rs:98-105`); PIC14E's `MIRRORED_SFRS` list also needs to grow from PIC14's `{0x00, 0x02, 0x03, 0x04, 0x0A, 0x0B}` to the full 0x00-0x0B block (INDF0/1, PCL, STATUS, FSR0L/H, FSR1L/H, BSR, WREG, PCLATH, INTCON all mirror on this core), or `Device::bank_of` will emit spurious `BANKSEL`s around FSR/BSR/WREG access. |
| 2K-word pages via `PCLATH<4:3>` | Same 2K-word pages, but `PCLATH` is 7 bits and `MOVLP k` loads it in one instruction (DS41364B §2.3) |
| `PCLATH` paging on every call and goto | Same, plus `BRA` and `BRW` for relocatable branches. **`[VERIFY]`, currently stated two ways and not reconciled:** section 1's own table said "±256 words" in revision 1; D-6 says "PC + 1 + signed 9-bit." A signed 9-bit displacement is a range of ±256 *words* around PC+1, so these may actually agree, but neither was checked against DS41364B and the document should not carry an unreconciled figure into P1. |
| No multiply instruction | Still none: no `MULWF`/`MULLW` in the instruction set (DS41364B Table 26-3) |
| `const` in flash via `RETLW` jump tables | Same `RETLW` mechanism in v1 (D-5); the FSR-to-flash mapping is a named follow-up, not v1 |
| 8-bit FSR + IRP bit, objects must fit one bank window | 16-bit FSR0/FSR1 with `ADDFSR`, `MOVIW`/`MOVWI` pre/post inc/dec and indexed `[k]INDFn`. **Corrected from revision 1:** the linear data region (0x2000-0x29AF) is not extra storage, it is an alternate, bank-spanning *address* for the same physical GPR bytes `ram_banks` already describes (DS41364B §2.5.2). It lets the compiler choose a linear-addressing encoding instead of a banked encoding when convenient; it does not let the allocator place an object anywhere it couldn't already place one physically. See D-2. |
| One interrupt vector, manual context save | One vector at 0x0004, **hardware context save** of W/STATUS/BSR/FSR0/FSR1/PCLATH to shadow registers, restored by `RETFIE` (DS41364B §4.1) |
| 4 banks | Up to 32 banks of 128 bytes (DS41364B §2.2) |

### What carries over unchanged

`irparse` and `ir` (LLVM IR text is not target-specific), `wholeprog`,
`callgraph` (checked directly: no device dependency at all;
`check_depth` takes its limit from the caller). `legalize` is target-
independent by construction, not because it is "already device-aware"
as revision 1 claimed: per ADR-014, every `mul`/`udiv`/... lowers to a
runtime `Inst::Call` regardless of backend, and the *recipe bodies*
differ per backend while *selection* does not. The PIC14 integer
spine's ALU/literal/bit emitters, the `PCLATH` paging machinery, the
`__mul_u8`/`__udiv_u8` shift-add routines, the soft-float routines, and
the `RETLW` const-table machinery all carry over with mnemonic-for-
mnemonic substitution into `isel-pic14e` (a third copy, D-1).

### What is adapted, not carried over untouched

**`crates/schedule` was missing from revision 1 entirely.** It is a
real, wired pipeline stage (ADR-027, epic-cc#210) that runs between
`isel` and `banking` on the `Core::Pic14` path
(`crates/driver/src/main.rs:340`), is device-parameterised
(`classify(device, asm)`, `schedule(device, asm)`), and reuses
`banking::operand_bank` and `banking::SKIP_OPS` by design. The pipeline
is eleven stages, not ten, once `schedule` is counted, and PIC14E needs
it: the doc-comment "sinks a W-and-flag-free excursion past its
successor" optimization is exactly the kind of skip-aware transform
this document's D-1 leans on `banking` to get right. `crates/fuzz`
needs an actual new arm, not just "the harness unchanged": it has five
`Core::Pic14e => panic!(...)` firewalls today and hard-codes per-core
RAM sizes in `run_pic` (`Pic14` 512 bytes, `Pic18` 4096, per ADR-016);
PIC14E's data space is 32×128 = 4096 addresses plus the linear alias,
so `run_pic` needs a third arm and `sim` needs a `Pic14e` core (P1).

### What is deleted or simplified, corrected from revision 1

**The manual ISR save/restore prologue is genuinely deleted**: the
hardware shadow registers do it (D-4).

**The `BANKSEL`/skip-window hazard is not deleted. Revision 1 said it
was; that was wrong, and the error mattered enough to hold the
document.** The hazard is not about instruction count or flags, it is
that inserting *any* instruction between a skip test and its target
changes what gets skipped. `crates/banking/src/lib.rs:69-76` defines
`SKIP_OPS`; `crates/alloc/src/lib.rs:139-146` states the consequence
directly: "the routine recipe loops are skip-sensitive (issue #6): a
BANKSEL the banking pass would insert changes the skip targets, so the
whole frame must live in ONE GPR bank." PIC18 escaped this because it
has real conditional branches (ADR-014: "PIC18 branches are absolute,
so a `MOVLB` between a test and its target is harmless"). **PIC14E has
no conditional branch** (only `BRA`/`BRW`, both unconditional), so it
keeps every classic-PIC14 skip idiom, and a single `MOVLB` breaks a
skip pair exactly as thoroughly as the two-instruction classic
`BANKSEL` does. PIC14E's escape hatch is also smaller than either
existing backend's: PIC18 has a 96-byte access bank plus a compiler-
reserved 16 bytes; PIC14E has the *same* 16 bytes of common RAM as
classic PIC14, already 14/16 spoken for. **The single-GPR-bank
routine-frame constraint survives into PIC14E, unchanged, and P2 must
design for it from the start; it is not something the port removes.**
This also means issue [#6](https://github.com/apojomovsky/epic-cc/issues/6)
is **not** obsoleted by this port (section 5 corrects the table that
previously said it was).

---

## 2. Decisions

### D-1: A third parallel backend crate, `isel-pic14e`; banking stays a post-isel text pass

**Decision:** `isel` stays as the classic PIC14 backend, `isel-pic18`
stays as the PIC18 backend, a new `isel-pic14e` crate holds the
Enhanced Mid-range backend, and **PIC14E bank tracking is a post-isel
text pass living in `crates/banking`, adapted for `BSR`/`MOVLB`
alongside the existing RP1:RP0 path, not in-isel tracking the way
`isel-pic18` tracks its access-bank state.** `crates/schedule` is
adapted the same way (device-parameterised, reusing its existing
skip-awareness). `iselcore` gains no new `Slot` variant in v1 (see
D-2): the linear-addressing design keeps `Slot::Direct(u16)` storing a
physical address, so nothing about `iselcore`'s existing contract
changes.

**Why banking is a post-isel pass here and an in-isel pass on
PIC18, stated as a decision rather than left as an unresolved "neither
this nor that" (which is what revision 1 actually said, in two places
that contradicted each other).** PIC18's isel-embedded BSR tracking
(`crates/isel-pic18`'s `Gen.bsr` field) is safe specifically *because*
PIC18 has no skip-sensitive idiom for it to land inside of: any
instruction can be inserted anywhere. PIC14E does not have that
freedom (section 1's corrected finding). The existing `crates/banking`
pass already carries the skip-safety machinery this needs
(`SKIP_OPS`, the exit-bank dataflow analysis at
`crates/banking/src/lib.rs:150-250`) because classic PIC14 has the
identical hazard. Reusing that machinery for a `BSR`/`MOVLB` variant is
lower-risk than re-deriving skip-safety inside a new `isel-pic14e`
emitter, which is exactly the kind of subtle correctness surface a
from-scratch implementation should not have to re-earn. The cost is
that `crates/banking` grows a second bank-tracking scheme instead of
staying purely RP1:RP0; that is accepted.

**ISA-surface argument for the crate split, an inventory rather than an
assertion.** The byte-oriented, bit-oriented and literal opcode
families (`ADDWF`/`SUBWF`/`IORWF`/... with the `d` bit,
`MOVLW`/`ADDLW`/... literals, `BCF`/`BSF`/`BTFSC`/`BTFSS`) are claimed
bit-identical to classic PIC14 (`[VERIFY]`, section 1); if that holds,
this is the majority of `isel`'s ~7,200 lines by instruction count, and
it is `isel`'s code, not `isel-pic18`'s. The paging machinery
(`PCLATH` dataflow, the M11 restore pair) is likewise `isel`'s shape
with `MOVLP` as a one-instruction load. The FSR/indirect machinery
genuinely diverges from both: `isel-pic18`'s `PLUSWn`-based indexing
(ADR-009: "no FSR auto-increment, no second FSR... exactly one
indirection register") is explicitly the opposite of what PIC14E's
`MOVIW`/`MOVWI` auto-inc/dec and dual FSR0/FSR1 provide, so nothing
about that part is actually shared with PIC18; the genuinely shared
piece is `iselcore::resolve_pointers`, already target-independent and
already called by both existing backends.

**Rejected, a `Target` trait over one generic `isel`.** Same objection
as the PIC18 port's D-1: the abstraction degrades into wrapper
functions that obscure the point of the port, and it refactors a
working, tested backend first.

**Rejected, one crate parameterised by `Device`, branching
internally.** Same scatter objection as PIC18's D-1.

**Considered and deferred rather than dismissed: extracting the
mul/div and soft-float recipe bodies into `iselcore`.** Revision 1
rejected this with "same reasoning as PIC18's D-1," which does not
transfer: PIC18's D-1 rejected sharing *because* PIC18's implementations
of those routines are meaningfully different (hardware `MUL` deletes a
loop body rather than shortening it), which is precisely the situation
where forking earns its cost. PIC14E's routines are, by this
document's own description, 1:1 transcriptions of the verified PIC14
bodies with a mnemonic substitution table, which is the situation where
forking is the *weaker* argument, not the stronger one. ADR-016 records
the real cost of forking recipe bodies: a fuzz-surfaced arithmetic bug
was fixed in `isel-pic18` only, and a third silent copy is a third
place the same bug can live uncaught. Kept as a third copy for v1
anyway, to bound this port's scope to the crate layout question alone,
but recorded here as a named, deliberately deferred follow-up rather
than a closed question, and worth revisiting the moment two of the
three copies diverge on a fixed bug.

### D-2: Static allocation in v1; the linear region is an addressing-mode choice, not a placement pool

**Decision, corrected from revision 1.** v1 ports the existing
call-graph overlay allocator unchanged. Recursion remains a compile
error. **The allocator continues to assign every object a physical
`(bank, offset)` address exactly as it does today; it never places
anything "in the linear region" as a distinct pool.** What PIC14E's
linear data region (DS41364B §2.5.2, FSR addresses 0x2000-0x29AF
aliasing the concatenated 80-byte GPR blocks of every bank) adds is a
second way to *address* an already-allocated physical byte range: a
fixed, architecture-level formula (`linear_addr = 0x2000 + bank * 80 +
offset`, `[VERIFY]` the exact stride and base against DS41364B) that
`isel-pic14e` may choose when emitting an access to an object whose
physical layout would otherwise need a bank switch mid-access (the
canonical case: a struct or array that straddles a bank boundary).
`iselcore::Slot::Direct(u16)` keeps storing the physical address
unchanged; choosing banked-vs-linear addressing for a given `Direct`
slot is an emission-time decision in `isel-pic14e`, analogous to
choosing between two equivalent instruction encodings, not a new kind
of storage location. This is what resolves revision 1's own §7 item
("P3's first design question", left open there): it is decided here,
in D-2, not deferred to P3.

**Consequence for D-3's device data.** Because the linear region is a
fixed architectural formula, not per-device data, it does not need a
`linear_ram` TOML field, an ADR-021-style oracle, or a `gen-device.py`
extractor (revision 1 proposed a field with none of those, which was
itself a defect: see D-3). It needs one constant, hard-coded once in
`isel-pic14e` for the `pic14e` core (the base address and per-bank
stride are architectural, not per-part), `[VERIFY]`ed against DS41364B
and cross-checked that it agrees for both the 1937 and 1939.

**Why the abstraction already exists.** `Slot` landed with the PIC18
port (P0, `iselcore`), and D-2's resolution above means PIC14E consumes
it genuinely unchanged, with no retrofitting and no new variant, which
was not actually true of revision 1's design despite revision 1
claiming `iselcore` was "unchanged."

### D-3: Device support is data, via the existing registry, corrected sketch

**Decision:** the 193x family lands as TOMLs under
`crates/device/devices/` (`p16f1937.toml`, `p16f1939.toml`, and the
rest of the family), generated by `scripts/gen-device.py`, which
already maps the EDC/ini architecture `16exxx`/`PIC14E` to `core =
"pic14e"` (checked directly: `scripts/gen-device.py:312-313`). The
firewall stays until the backend lands.

**Rationale.** ADR-019 already settled device-as-data and the
file-per-device TOML registry; `gen-device.py` already knows the
`pic14e` core string, so this is genuinely a data-only step for
everything except the two items named below.

**Not actually data-only, corrected from revision 1's D-3, which
called it that.** Two real code changes gate P0, and belong in P0's
ticket rather than surfacing later as surprises:

1. **The config-word byte address in the sketch was wrong by a factor
   that matters.** PIC14 config addresses are word addresses that get
   doubled into a byte address (`crates/device/devices/p16f877a.toml`
   uses `base_byte_addr = 0x400E` for word `0x2007`, confirmed against
   `scripts/gen-device.py:609`). Revision 1's sketch wrote `0x8007  #
   word address, 14-bit words`, which does not double to itself; the
   corrected value below is `0x1000E`, still `[VERIFY]` against
   DS41364B for whether `0x8007` is even the right word address for
   this core.
2. **Two config words do not fit the existing PIC14 hex-emission
   path.** `crates/driver/src/main.rs:396-405`'s PIC14 arm writes
   exactly one config word through `asm::to_hex`, whose extended-
   linear-address record hard-codes the upper 16 bits to zero
   (`crates/asm/src/lib.rs:762-764`). A second config word at a byte
   address near `0x1000E` needs the `to_hex_regions` path PIC18 already
   uses (`crates/asm/src/lib.rs:806`). This is driver work that belongs
   in P0, not an implicit assumption.

**Explicit preconditions, not yet checked, both named in the approval-
status note above and repeated here because they gate whether P0 can
even open:**

- **DS41364B is not vendored.** `vendor/README.md`'s datasheet table
  lists DS39582, DS33023, DS52053, DS33014; PIC16F193X's DS41364B is
  not among them. Every `[VERIFY]` in this document has no checkable
  target in this repo today. Get it into `vendor/` before P0 starts,
  not during it.
- **Whether `gputils` 1.5.2 knows `-p p16f1937` and ships a `.lkr` for
  it has not been checked.** ADR-021's own "Revisit if" clause names
  this exact failure mode: a supported part with no `.lkr`, where "the
  flash half fails outright because the probe cannot run." P1's entire
  acceptance criterion is a `gpasm -p p16f1937` byte-for-byte match; if
  gputils does not know this part, P1 as scoped cannot run, and P0's
  cross-check gate (built on the same tool) is uncovered too. This is a
  five-minute check and should be the first thing whoever picks up P0
  does, before writing the TOML.

The 1937/1939 TOMLs otherwise need `[VERIFY]` against DS41364B for:
flash size (8192/16384 words), SRAM (512/1024 bytes), 16-level stack,
single vector at 0x0004, the corrected config word address(es), and the
bank map (32 banks × 128 bytes, 16 common bytes at 0x70-0x7F, and the
corrected `MIRRORED_SFRS` set from section 1: the full 0x00-0x0B block,
not classic PIC14's subset).

### D-4: Interrupts: single vector, hardware context save

**Decision:** target the single vector at 0x0004 with the hardware
shadow-register context save. No priority model exists on this core
(no `IPEN`), so there is no compatibility-mode question like PIC18's
P5 had.

**Rationale.** DS41364B §4.1: on interrupt entry the hardware saves
W/STATUS/BSR/FSR0/FSR1/PCLATH to shadow registers and restores them on
`RETFIE`. The ISR needs no manual save/restore prologue, which is
simpler than both existing backends (PIC14 saves manually, PIC18 has
the two-vector priority question). The `epic_dispatch_all_irqs`
fan-out shape carries over from `pic16f193x-hal`'s implementation if
that HAL turns out to be real rather than a placeholder (see the
target-choice note at the top of this document): read INTCON/PIR1/
PIR2/PIR3 once, dispatch only the sources whose bits are set, each
handler clears its own flag.

One consequence to record: because the shadow registers restore on
`RETFIE`, any W/STATUS/BSR/FSR/PCLATH value the ISR leaves behind is
lost. The compiler's ISR codegen must not rely on values surviving
across the ISR boundary, and the `_isr` frame copies (the PIC18 P5
mechanism) apply unchanged.

**Not yet asked: the ADR-026 question.** PIC18 needed `-fpack-struct`
in the clang invocation (`crates/driver/src/main.rs:157`,
`packed_structs: device.core == device::Core::Pic18`). PIC14E likely
inherits PIC14's natural-alignment answer (`false`) since it inherits
PIC14's data model, but this document should say so explicitly rather
than let it default silently; add to section 3's clang-invocation note
and confirm before P2.

### D-5: `const` in flash via `RETLW` in v1; the FSR-flash mapping is a follow-up

**Decision:** v1 ports the classic PIC14 `RETLW` const-table machinery
unchanged. The PIC14E-native FSR-to-flash mapping (DS41364B §2.5.3:
setting bit 7 of FSRnH maps the FSR to program flash, read through
`MOVIW`) is a documented follow-up, not v1.

**Rationale.** The `RETLW` machinery is proven, carries over with no
changes, and de-risks the port. The FSR-flash mapping is the PIC14E
analog of PIC18's `TBLRD`: it would delete the 256-byte window and
511-byte ceiling the same way. But it is a new mechanism with its own
simulator and assembler surface, and the port already has enough new
surface. Land `RETLW` first, then replace it when the FSR machinery is
settled.

### D-6: The `MOVLP`/`PCLATH` paging pass carries over from `isel`

**Decision:** the PIC14E paging pass is `isel`'s PCLATH dataflow with
`MOVLP k` as the load instruction. `BRA`/`BRW` are used where the
relative form fits, which reduces the number of pages that need
management.

**Rationale.** The 1937's 8192 words is four 2K-word pages, the same
page geometry as the 877A; the 1939's 16384 words is eight. The
`PCLATH<4:3>` -> `PC<12:11>` mapping for `CALL`/`GOTO` is identical to
classic PIC14 (DS41364B §2.3.2). `MOVLP` loads all 7 PCLATH bits in
one instruction, which is strictly simpler than the classic
`MOVLW`+`MOVWF PCLATH` pair. `BRA` and `BRW` are new and give the
backend relocatable branches, which the peephole pass can use to elide
page management on intra-page branches. The exact `BRA` range is the
`[VERIFY]` item flagged in section 1; whatever it turns out to be, both
figures currently in this document describe the same instruction, not
two different ones, so this decision does not change shape either way.

---

## 3. Architecture

### Crate layout after the port

```
crates/
  ir/            unchanged   the IR data model
  irparse/       unchanged   LLVM IR text parser
  wholeprog/     unchanged   module merging
  callgraph/     unchanged   call graph, cycle detection, depth check
  legalize/      unchanged   target-independent by construction (ADR-014)
  alloc/         unchanged   physical (bank, offset) allocation only; D-2
  device/        adapted     p16f1937.toml etc.; two driver-side gaps, D-3
  iselcore/      unchanged   Slot, ssa_key, parse_map, Base, resolve_pointers
  isel/          unchanged   the classic PIC14 backend
  isel-pic18/    unchanged   the PIC18 backend
  isel-pic14e/   NEW         the Enhanced Mid-range backend; linear-address
                             emission decision lives here (D-2)
  schedule/      adapted     device-parameterised; PIC14E skip-aware classify
  banking/       adapted     RP1:RP0 path kept; BSR/MOVLB path added (D-1)
  peephole/      adapted     PIC14 PCLATH path kept; PIC14E pass is separate
  asm/           adapted     PIC14 encoder kept; PIC14E encoder added
  sim/           adapted     PIC14 core kept; PIC14E core added
  driver/        adapted     selects the pipeline by device; two hex-path
                             and clang-invocation gaps, D-3/D-4
  fuzz/          adapted     new Pic14e arm and RAM size, not just a flag
```

`alloc` moves from revision 1's "adapted: linear data region as a
placement option" to **unchanged**, which is the point of D-2's
correction: the allocator never learns about the linear region at all.

### The device profile

The 1937 TOML follows the existing schema (ADR-019). Sketch, every
constant `[VERIFY]` against DS41364B once it is vendored (see D-3):

```toml
name = "p16f1937"
core = "pic14e"
flash_words = 8192          # DS41364B Table 1-1
ram_banks = [ ... ]         # 32 banks x 128 bytes, GPR blocks per bank
common_ram = [0x0070, 0x007F]
stack_depth = 16            # DS41364B section 2.4
interrupt_vectors = [0x0004]

[config]
base_byte_addr = 0x1000E    # corrected from revision 1's 0x8007; word
                             # address doubled, per the p16f877a.toml
                             # precedent (0x2007 -> 0x400E). [VERIFY]
                             # 0x8007 is even the right PIC14E word
                             # address; this only fixes the units bug.
num_bytes = 4                # CONFIG1 + CONFIG2; needs to_hex_regions
                              # in the driver, D-3 item 2

[provenance]
# required by build.rs (ADR-021); fill from the DFP source used
```

There is no `linear_ram` field: D-2 makes the linear region an
architectural constant inside `isel-pic14e`, not per-device data, which
also removes the problem revision 1 had of proposing a device field
with no oracle and no `gen-device.py` extractor.

### Data flow

The pipeline is eleven stages (`schedule` was missing from revision
1's count of ten). The branch is at exactly one place, in the driver,
and it selects a backend rather than threading a boolean through it.

```
.c -> clang -> .ll -> irparse -> wholeprog -> legalize -> callgraph -> alloc
                                                                        |
                     device selects: isel | isel-pic18 | isel-pic14e
                                                                        |
                                          schedule -> banking -> peephole -> asm -> .hex
```

(PIC18 skips `schedule`, `banking` and `peephole` entirely today,
per `crates/driver/src/main.rs:359`; PIC14E takes the PIC14-shaped
path through all three, adapted per D-1.)

---

## 4. Phases

Nine phases, front-loaded on de-risking, mirroring the PIC18 port's
phase list as the effort reference. Each has an acceptance criterion
that is a test, not a judgement, except where noted below as still
needing one.

**Before P0 opens:** confirm DS41364B is vendored, and confirm gputils
1.5.2 recognizes `-p p16f1937` and ships its `.lkr` (D-3). Neither is a
phase; both are go/no-go gates on the whole plan.

| Phase | Deliverable | Acceptance |
|---|---|---|
| **P0** | 193x device TOMLs via `gen-device.py`; the two driver-side hex-path and config-address fixes (D-3); firewall stays. No backend codegen. | `gen-device --check` clean; the TOMLs validate through `build.rs` including `[provenance]`; the driver still refuses `pic14e` with the existing message; the corrected config hex path is exercised by a unit test independent of the firewall. |
| **P1** | PIC14E `asm` encoder and `sim` core, including `MIRRORED_SFRS` for the 0x00-0x0B block. Hand-written `.asm` inputs only, no codegen. | `gpasm -p p16f1937` byte-for-byte HEX match, plus simulator tests per instruction group (the new ASRF/LSLF/LSRF, MOVLB/MOVLP, BRA/BRW/CALLW, ADDFSR/MOVIW/MOVWI, and a negative test confirming `OPTION`/`TRIS`/RP0-RP1-IRP are inert or rejected, not silently miscompiled). |
| **P2** | Integer spine: `isel-pic14e`, BSR/MOVLB banking via the adapted `crates/banking` pass (D-1), `Slot::Direct` unchanged (D-2). The single-GPR-bank routine-frame constraint (section 1) is designed for here, not deferred. | `add.c`, `scalar.c`, `overlay.c`, `banked.c`; a routine-recipe-shaped skip-idiom test confirming no `MOVLB` lands inside a skip pair |
| **P3** | Pointers, arrays, structs via FSR0/1; linear addressing (D-2) used for objects that straddle a bank. | `ptr_probe.c`, `array.c`, `structs.c`, `banked_ptr.c` extended with a struct/array that spans two banks; the emitted `.asm` is inspected to confirm linear addressing was chosen for the spanning case and banked addressing otherwise, not just that the simulator produces the right value |
| **P4** | `const` in flash via `RETLW` (carried over). | `const_table.c`, `ptr_probe.c`; the 511-byte ceiling stays (D-5) |
| **P5** | Interrupts: single vector, hardware context save, `_isr` frame copies. | `interrupt.c`, `interrupt_gate.c` for simulated behavior, plus a static assertion on the emitted `.asm` that no manual save/restore sequence was generated (a simulation pass alone cannot distinguish "hardware saved it" from "we saved it and it happened to work") |
| **P6** | 32-bit `long`, mul/div routines (carried over from PIC14, third copy per D-1). | `long.c`, `muldiv.c`, `interrupt_mul.c` |
| **P7** | Soft-float: f32 routines (1:1 port of the PIC14 bodies, third copy per D-1). | `float.c` (out1=0x3F99999A, out2=0x41100000, out3=0x3EAAAAAB) |
| **P8** | Fuzz gate: device-threaded differential runner on PIC14E, including the new `Pic14e` RAM-size arm in `run_pic` (section 1). | `pic14e.rs` fast (8) and full corpora (200, 50, 50) clean on the PIC14E sim via `--device` |

**P0 deserves emphasis, with the caveat added above.** It is close to
pure data, with the existing suite as its oracle for everything except
the two driver-side fixes named in D-3, which are real code and belong
in P0 rather than surfacing later.

**P1 before P2 is deliberate.** Same reasoning as the PIC18 port: a P2
failure must never be ambiguous between "chose the wrong instruction"
and "emitted the wrong bits." The new instructions (the shifts, the
FSR moves, the branches) are exactly where that ambiguity would bite.

**P5 is simpler than either existing backend's**, on the ISR-boundary
question specifically. No manual save (PIC14), no priority question
(PIC18). The hardware context save is the whole story, which is why
its acceptance criterion above adds a static check: the simplicity
claim is about the *source* of correctness, not just its *outcome*.

**v1 stops at P8, deliberately smaller than "parity with PIC14 today."**
See the rescoped definition of done above. A PIC14E-parity-phase-2
follow-up, mirroring PIC18's own P4-P8 extension, is the natural home
for the explicitly-out-of-scope list once P8 lands.

---

## 5. Prerequisites and backlog interaction

### Must land before P2

**None hard.** The PIC18 port's blockers (#9, #10, #14) all landed
during that port. `Global.addr` is 16-bit, constant folding exists, and
the differential fuzzer is device-threaded.

### Obsoleted by the port for PIC14E's own purposes; corrected from revision 1

| Issue | Status |
|---|---|
| [#7](https://github.com/apojomovsky/epic-cc/issues/7) FSR globals inside bank windows | Still obsoleted for PIC14E's purposes: PIC14E's FSRs are genuinely 16-bit hardware, unlike classic PIC14's 8-bit-plus-IRP, so this port does not need #7's fix to address objects across banks. |
| [#6](https://github.com/apojomovsky/epic-cc/issues/6) Bank-0 restriction on routine slots | **Not obsoleted. Revision 1 was wrong about this** (section 1's corrected finding): the single-GPR-bank routine-frame constraint survives on PIC14E because the skip-window hazard survives. #6 remains open and relevant to both backends. |

Both remain worth doing **for the PIC14 backend on its own merits**
regardless of this table; the claim here is only about ordering
relative to this port.

### Either order

[#11](https://github.com/apojomovsky/epic-cc/issues/11) (IEEE754 edge
cases): the corpus is reusable for the PIC14E float routines, same
argument as the PIC18 port made.

---

## 6. Testing strategy

The port inherits the four verification layers rather than inventing
any.

1. **Our own simulator**, extended with a PIC14E core. Deterministic,
   embeddable in `cargo test`.
2. **`gpasm` byte-for-byte cross-check**, against `-p p16f1937`, gated
   on the precondition in D-3 that gputils actually supports this part.
   This is P1's entire acceptance criterion and the reason P1 precedes
   P2.
3. **The e2e acceptance programs**, recompiled for PIC14E. They are
   the parity definition, phase by phase, in the P2-P7 table above,
   scoped per the rescoped definition of done at the top.
4. **Differential fuzzing** against host clang, with the seed corpora.

Two additions specific to a port, both inherited from the PIC18 port:

- **The PIC14 suite is a regression oracle for P0.** Keeping every
  existing test passing through the driver's two new hex-path/config-
  address changes (D-3) is the strongest available evidence those
  changes did not disturb the classic PIC14 or PIC18 paths.
- **Cross-target differential.** The same C source compiled for both
  targets must produce the same observable results in the simulator.

**`pic16f193x-hal` as a cross-check pillar needs confirming to exist
before it is counted on.** Revision 1 cited its `docs/ARCHITECTURE.md`
(BSR auto-banking of literal SFR tokens, the FSR1:INDF1 RMW trap, the
`movlb 1` + `iorwf PIE1,f` fix shape) as "the reference for the device
TOML's SFR table." That may be entirely accurate, but this document
cannot see epic-hal's repository, and epic-cc's own
`docs/31-ecosystem-integration-design.md` D-1 currently defers
`pic16f193x-hal` for the stated reason that no backend exists yet.
Confirm which is true (a real, XC8-verified HAL already exists to
reference, or it does not and this pillar is aspirational) before
relying on it, and reconcile whichever is true with `docs/31`.

---

## 7. Risks and open questions

**Every `[VERIFY]` item above**, now unresolvable in this repo until
DS41364B is vendored (D-3): memory map, bank layout, the linear-region
formula's exact base/stride, stack depth, vector address, config word
address(es), flash size, the bit-identical-encoding claim underpinning
D-1, and the `BRA` range figure.

**The FSR address space has three regions.** Traditional data memory
(0x000-0xFFF), the linear alias (0x2000-0x29AF, D-2), and program flash
(bit 7 of FSRnH set, 0x8000+), with reserved gaps between (DS41364B
§2.5). The simulator must model all three and the one-extra-cycle cost
of flash access. Getting this boundary wrong is a whole class of P1
bugs, which is precisely why P1 is gated on a byte-for-byte `gpasm`
match.

**The `MOVIW`/`MOVWI` pre/post inc/dec forms.** `++INDFn`, `--INDFn`,
`INDFn++`, `INDFn-`, and the indexed `[k]INDFn` form each have a
distinct encoding and a distinct FSR side effect (DS41364B Table 26-3).
The assembler's encoding and the simulator's FSR update must agree;
this is the PIC14E analog of the PIC18 two-word-instruction risk.

**The shadow-register interrupt model.** The hardware saves
W/STATUS/BSR/FSR0/FSR1/PCLATH on entry and restores on `RETFIE`
(DS41364B §4.1). The compiler must not rely on ISR-side register
values surviving, and the `_isr` frame copies must be disjoint from
the main frames, same as PIC18's P5. The one thing to verify: re-read
§4.1 to confirm the shadow save covers every interrupt entry on this
core, which has no priority levels.

**The clang side is assumed unchanged, including one question revision
1 didn't ask.** `-target msp430` remains the datalayout proxy, same as
PIC18 (8-bit `char`, 16-bit `int`). `[VERIFY]` that the PIC14E pointer
width (16-bit FSR-based, like PIC18's) does not argue for a different
proxy, and answer the `-fpack-struct`/ADR-026 question explicitly
(D-4) rather than let PIC14E inherit PIC14's `false` by omission.

**`MIRRORED_SFRS` needs a PIC14E-specific set.** Section 1 and D-3 name
this; repeated here because it is a P1/P2 boundary concern: get it
wrong and the symptom is a spurious `BANKSEL` around FSR/BSR/WREG
access, which is the kind of bug that only shows up once P2's codegen
starts exercising those registers, not in P1's hand-written `.asm`
tests.

**Resolved from revision 1, kept here for visibility.** The linear
region's interaction with the allocator was revision 1's open "P3's
first design question." It is resolved in D-2 above: the allocator
never sees the linear region, and `isel-pic14e` chooses linear-vs-
banked addressing per access at emission time. If P3 finds a case D-2
did not anticipate, that is a design gap in D-2 to fix directly, not a
question to reopen from scratch.
