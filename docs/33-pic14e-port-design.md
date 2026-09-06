# 33: PIC14E (Enhanced Mid-range) port design

> **Approval status:** revision 3. Revision 2 addressed an independent
> review (2026-09-05) that held revision 1 rather than approving it: a
> correctness-class error in the document's own reasoning (the
> skip-window hazard does not disappear on this core, it survives; see
> D-1 and the table in section 1), an architectural question
> incorrectly deferred to P3 that actually gates P0 (the linear-region
> design, resolved in D-2), and a set of factual errors in the
> device-data sketch (D-3). Revision 3 vendors and reads the actual
> datasheets, which were not available in this repo when revision 2 was
> written: **both external preconditions are now resolved.** `vendor/
> microchip/datasheets/` now holds `DS41364E.pdf` (PIC16F1934/6/7,
> revision 2 named the wrong document number, `DS41364B`; the correct,
> current one is `E`), `DS40001574D.pdf` (PIC16F1938/9, a genuinely
> **separate** document, not the same one as the 1934/6/7 family, a
> distinction revision 2 also missed), and `DS80000479.pdf` (the family
> silicon errata). `gputils` support for both target parts was already
> confirmed in revision 2. Every `[VERIFY]` item this revision could
> check against those documents is resolved below, one of them
> correcting a claim revision 2 itself introduced (OPTION/TRIS were not
> actually removed on this core; see section 1). **Still pending user
> approval**, on the design decisions, not the facts. This document is
> the design of record for
> [issue #228](https://github.com/apojomovsky/epic-cc/issues/228). The
> implementation plan derives from it and does not exist yet.

**Target:** the PIC16F1937 / PIC16F1939 family (the 1933/1934/1936/1938
share the core with less flash and RAM; the PIC12F1xxx parts share the
core once it exists, noted but not scoped here). The 1937/1939 are
popular hobbyist Enhanced Mid-range parts, and `pic16f193x-hal` is a
real, substantial reason to pick this family, confirmed by reading
epic-hal directly rather than assuming: `pic16f193x-hal/docs/
ARCHITECTURE.md` records real-target builds passing for all six family
parts under XC8, disassembly-level codegen probing of the actual
compiled firmware (the FSR1:INDF1 indirect-addressing pattern this core
uses for runtime SFR access, and a real RMW trap it found and fixed),
and `mdb`-confirmed register readback for at least one peripheral.
epic-cc's own `docs/31-ecosystem-integration-design.md` D-1 calling
`pic16f193x-hal` "deferred" is not in tension with this: that decision
is about epic-cc's own *compiler backend* not existing yet for this
core, not about the HAL itself being unfinished or a placeholder. This
port is exactly what closes that gap.

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
this table is now confirmed against the actual datasheets (DS41364E
for 1934/6/7, DS40001574D for 1938/9, both in `vendor/microchip/
datasheets/`, see the approval-status note); citations below name the
real section and table, not the guessed ones revision 2 carried
forward from general PIC14E knowledge before the documents were
vendored.

| PIC14 constraint | PIC14E |
|---|---|
| 14-bit instruction words | Same 14-bit words. **Confirmed**: the byte-oriented, bit-oriented and literal opcode families are bit-identical encodings to the classic set (DS41364E Table 29-3, page 367; e.g. `ADDWF f,d` = `00 0111 dfff ffff`, `BCF f,b` = `01 00bb bfff ffff`, matching classic PIC14 exactly). **Corrected, this revision got it wrong first:** `OPTION` and `TRIS f` are *not* removed. Both are listed as real instructions in DS41364E Table 29-3 under "Inherent Operations" (`OPTION` = `00 0000 0110 0010`, `TRIS f` = `00 0000 0110 0fff`), and function as documented, alternate ways to write `OPTION_REG`/`TRISx` directly. What *is* confirmed gone: STATUS bits 7-5 (classic PIC14's IRP/RP1/RP0) are "Unimplemented: Read as '0'" on this core (DS41364E Register 3-1), so a classic-PIC14 `BCF/BSF STATUS,5/6/7` the backend might inherit assembles cleanly and does nothing; this is still worth a P1 negative test, just not the OPTION/TRIS one revision 2 proposed. |
| 8-level hardware stack | 16-level (DS41364E page 3, "16-Level Deep Hardware Stack"). Not addressable for compiler purposes in v1. |
| Bank via RP1:RP0 bits in STATUS | Dedicated `BSR` register, `MOVLB k` (DS41364E Table 29-3: `MOVLB k` = `00 0000 001k kkkk`, a 5-bit literal, addressing all 32 banks) |
| 16 bytes of `BANKSEL`-free common RAM (0x70-0x7F) | Same 16 common bytes, reachable from any bank (DS41364E section 3.2.4, "16 bytes of common RAM accessible from all banks"). Today's PIC14 backend already spends 14 of those 16 bytes on fixed scratch/retval/ISR-save (`crates/driver/src/report.rs:98-105`); PIC14E's `MIRRORED_SFRS` list also needs to grow from PIC14's `{0x00, 0x02, 0x03, 0x04, 0x0A, 0x0B}` to the full 0x00-0x0B block. **Confirmed** directly against DS41364E's Table 3-3 memory map (page 30): every bank's first 12 bytes are `INDF0, INDF1, PCL, STATUS, FSR0L, FSR0H, FSR1L, FSR1H, BSR, WREG, PCLATH, INTCON`, exactly the reviewer's proposed set, word for word. Get this wrong and `Device::bank_of` emits spurious `BANKSEL`s around FSR/BSR/WREG access. |
| 2K-word pages via `PCLATH<4:3>` | Same 2K-word pages, `PCLATH` is 7 bits, `MOVLP k` loads it in one instruction (DS41364E Table 29-3: `MOVLP k` = `11 0001 1kkk kkkk`) |
| `PCLATH` paging on every call and goto | Same, plus `BRA` and `BRW` for relocatable branches. **Confirmed, and the two figures were never actually in conflict:** DS41364E Table 29-3 gives `BRA k` = `11 001k kkkk kkkk`, a 9-bit signed literal (1 bit in the opcode nibble plus 8 more), which is exactly a ±256-word range around PC+1. "±256 words" and "signed 9-bit" describe the same encoding at two levels of precision; there was nothing to reconcile once the table was actually read. `BRW` = `00 0000 0000 1011`, a fixed encoding with no literal field, since the offset comes from `W`. |
| No multiply instruction | Still none: no `MULWF`/`MULLW` in DS41364E Table 29-3 |
| `const` in flash via `RETLW` jump tables | Same `RETLW` mechanism in v1 (D-5); the FSR-to-flash mapping is a named follow-up, not v1 |
| 8-bit FSR + IRP bit, objects must fit one bank window | 16-bit FSR0/FSR1 with `ADDFSR`, `MOVIW`/`MOVWI` pre/post inc/dec and indexed `[k]INDFn` (DS41364E Table 29-3, "C-Compiler Optimized" group). The linear data region (0x2000-0x29AF, **confirmed** exact, DS41364E section 3.5.2 and Figure 3-11) is not extra storage, it is an alternate, bank-spanning *address* for the same physical GPR bytes `ram_banks` already describes. It lets the compiler choose a linear-addressing encoding instead of a banked encoding when convenient; it does not let the allocator place an object anywhere it couldn't already place one physically. See D-2 for the confirmed formula. |
| One interrupt vector, manual context save | One vector at 0x0004, **hardware context save** of W, STATUS (except TO/PD), BSR, FSR0, FSR1, PCLATH to shadow registers, restored by `RETFIE` (DS41364E section 7.5, "Automatic Context Saving", **confirmed** word for word against the D-4 claim). The shadow registers themselves live in Bank 31 and are readable/writable, useful if an ISR ever needs to see or override what will be restored. |
| 4 banks | 32 banks of 128 bytes (DS41364E section 3.2: "The data memory is partitioned in 32 memory banks with 128 bytes in a bank", confirmed, each bank = 12 core registers + up to 20 SFRs + up to 80 bytes GPR + 16 bytes common RAM) |

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
linear data region (DS41364E section 3.5.2, FSR addresses 0x2000-0x29AF
aliasing the concatenated 80-byte GPR blocks of banks 0-30, confirmed
against Figure 3-11) adds is a second way to *address* an already-
allocated physical byte range: a fixed, architecture-level formula,
`linear_addr = 0x2000 + bank * 80 + (physical_offset - 0x20)` for
`physical_offset` in a bank's 80-byte GPR window (0x20-0x6F within the
bank; the 16 bytes of common RAM are explicitly excluded from the
linear region per DS41364E section 3.5.2, and bank 31 is not included
either, the region covers exactly 31 banks x 80 bytes = 2480 bytes,
0x2000 to 0x29AF), confirmed identical in DS40001574D for the 1938/9,
so this is a per-core constant, not a per-part one. `isel-pic14e` may
choose this addressing when emitting an access to an object whose
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
`isel-pic14e` for the `pic14e` core; confirmed above that the base
address, stride and bank count agree exactly between DS41364E (1937)
and DS40001574D (1939).

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
firewall stays until the backend lands. **The family spans two
datasheets, not one**, a distinction revision 1 and 2 both missed by
citing a single document number for the whole family: PIC16F1934/6/7
(including the 1937) is DS41364E; PIC16F1938/9 (including the 1939) is
the separately-numbered DS40001574D. Both are now in `vendor/
microchip/datasheets/`, along with the family silicon errata,
DS80000479.

**Rationale.** ADR-019 already settled device-as-data and the
file-per-device TOML registry; `gen-device.py` already knows the
`pic14e` core string, so this is genuinely a data-only step for
everything except the two items named below.

**Not actually data-only, corrected from revision 1's D-3, which
called it that.** Two real code changes gate P0, and belong in P0's
ticket rather than surfacing later as surprises:

1. **The config-word byte address in the sketch was wrong by a factor
   that matters, though the underlying word address itself was right.**
   PIC14 config addresses are word addresses that get doubled into a
   byte address (`crates/device/devices/p16f877a.toml` uses
   `base_byte_addr = 0x400E` for word `0x2007`, confirmed against
   `scripts/gen-device.py:609`, whose doubling rule is `cwords_are_bytes
   = core == "pic18"`, so `pic14e` gets the same doubling as `pic14`
   automatically, no new code needed there). **Confirmed against
   DS41364E section 4.1:** Configuration Word 1 is genuinely at word
   address `0x8007`, Configuration Word 2 at `0x8008`, exactly what
   revision 1 wrote. The bug was purely the units label: revision 1's
   sketch wrote `base_byte_addr = 0x8007`, which does not double to
   itself. The corrected value is `0x1000E`.
2. **Two config words do not fit the existing PIC14 hex-emission
   path.** `crates/driver/src/main.rs:396-405`'s PIC14 arm writes
   exactly one config word through `asm::to_hex`, whose extended-
   linear-address record hard-codes the upper 16 bits to zero
   (`crates/asm/src/lib.rs:762-764`). A second config word at a byte
   address near `0x1000E` needs the `to_hex_regions` path PIC18 already
   uses (`crates/asm/src/lib.rs:806`). This is driver work that belongs
   in P0, not an implicit assumption.

**Both external preconditions from earlier revisions are now resolved,
repeated here for the record:**

- **Datasheets vendored and read.** `DS41364E.pdf` (1934/6/7) and
  `DS40001574D.pdf` (1938/9) are in `vendor/microchip/datasheets/`,
  along with the errata `DS80000479.pdf`. Every fact this document
  states as confirmed was checked against them directly.
- **Resolved, checked directly in the dev image.** `gpasm -l14e` lists
  `p16f1937` and `p16f1939` (and the rest of the family) by name, and
  `/usr/local/share/gputils/lkr/16f1937_g.lkr` exists, alongside a
  `p16f1937.inc` header and gputils' own generated SFR/RAM/config/
  feature HTML reference pages under
  `/usr/local/share/doc/gputils-1.5.2/html/`. ADR-021's "Revisit if"
  failure mode (a supported part with no `.lkr`) does not apply here.
  P1's `gpasm -p p16f1937` acceptance criterion can run. gputils' own
  generated HTML pages are a second, independent cross-check for the
  device TOMLs once P0 starts writing them, alongside the datasheets.

The 1937/1939 TOMLs need, per-part: flash size (8192 words for the
1937 per DS41364E's family table; 16384 for the 1939, `[VERIFY]`
against DS40001574D's equivalent table, not yet checked in this
revision), SRAM (512/1024 bytes per the same tables), and the config
word values proper (the bit-field *meanings*, Register 4-1 onward in
DS41364E, are extensive and were not transcribed here; whoever writes
the actual TOML should read that register description directly rather
than work from this summary). Confirmed and shared across both parts,
not needing further per-part checking: 16-level stack, single vector at
0x0004, the corrected config word address(es) (0x8007/0x8008 words,
0x1000E/0x10010 bytes), and the bank map (32 banks x 128 bytes, 16
common bytes at 0x70-0x7F, `MIRRORED_SFRS` the full 0x00-0x0B block).

### D-4: Interrupts: single vector, hardware context save

**Decision:** target the single vector at 0x0004 with the hardware
shadow-register context save. No priority model exists on this core
(no `IPEN`), so there is no compatibility-mode question like PIC18's
P5 had.

**Rationale.** **Confirmed**, DS41364E section 7.5 ("Automatic Context
Saving"): on interrupt entry the hardware saves W, STATUS (except TO
and PD), BSR, FSR0, FSR1 and PCLATH to shadow registers and restores
them on `RETFIE`; the shadow registers themselves live in Bank 31 and
are readable/writable. The ISR needs no manual save/restore prologue,
which is
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
unchanged. The PIC14E-native FSR-to-flash mapping (**confirmed**,
DS41364E section 3.5.3, "Program Flash Memory": setting the MSb of
FSRnH maps the FSR to program flash, the lower 15 bits address it,
only the low 8 bits of each location are readable through `INDF`, one
extra instruction cycle per access, and it is read-only, writing flash
this way is not possible) is a documented follow-up, not v1.

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
page geometry as the 877A; the 1939's 16384 words is eight. `MOVLP`
loads all 7 PCLATH bits in one instruction (**confirmed**, DS41364E
Table 29-3), which is strictly simpler than the classic
`MOVLW`+`MOVWF PCLATH` pair. `BRA` and `BRW` are new and give the
backend relocatable branches, which the peephole pass can use to elide
page management on intra-page branches: `BRA`'s 9-bit signed literal
(section 1, confirmed) covers a ±256-word jump with no page management
at all. The `PCLATH<4:3>` behavior on `CALL`/`GOTO` itself was not
re-derived from the datasheet in this revision; DS41364E's "PCL and
PCLATH" section (3.3) is the place to check it, `[VERIFY]` still open.

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

The 1937 TOML follows the existing schema (ADR-019). Sketch, values
confirmed against DS41364E except where noted:

```toml
name = "p16f1937"
core = "pic14e"
flash_words = 8192          # DS41364E family table, page 4
ram_banks = [ ... ]         # 32 banks x 128 bytes, GPR blocks per bank
common_ram = [0x0070, 0x007F]
stack_depth = 16            # DS41364E page 3, "16-Level Deep Hardware Stack"
interrupt_vectors = [0x0004]

[config]
base_byte_addr = 0x1000E    # word 0x8007, confirmed DS41364E section 4.1,
                             # doubled per the existing PIC14 convention
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

**Before P0 opens:** both external preconditions from earlier revisions
are resolved (D-3): the datasheets are vendored and read, and gputils
support for both target parts is confirmed. Nothing external blocks
starting P0 once this document itself is approved.

| Phase | Deliverable | Acceptance |
|---|---|---|
| **P0** | 193x device TOMLs via `gen-device.py`; the two driver-side hex-path and config-address fixes (D-3); firewall stays. No backend codegen. | `gen-device --check` clean; the TOMLs validate through `build.rs` including `[provenance]`; the driver still refuses `pic14e` with the existing message; the corrected config hex path is exercised by a unit test independent of the firewall. |
| **P1** | PIC14E `asm` encoder and `sim` core, including `MIRRORED_SFRS` for the 0x00-0x0B block. Hand-written `.asm` inputs only, no codegen. | `gpasm -p p16f1937` byte-for-byte HEX match, plus simulator tests per instruction group (the new ASRF/LSLF/LSRF, MOVLB/MOVLP, BRA/BRW/CALLW, ADDFSR/MOVIW/MOVWI, `OPTION`/`TRIS` writing `OPTION_REG`/`TRISx` correctly since they are real instructions here, and a negative test confirming STATUS bits 5-7 (RP0/RP1/IRP) read as 0 and a `BCF`/`BSF` against them is inert rather than silently miscompiled). |
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
2. **`gpasm` byte-for-byte cross-check**, against `-p p16f1937`; gputils
   support for this part is confirmed (D-3). This is P1's entire
   acceptance criterion and the reason P1 precedes
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

**`pic16f193x-hal` as a cross-check pillar, confirmed by reading
epic-hal directly.** Its `docs/ARCHITECTURE.md` is real, XC8-verified
material, not aspirational: real-target builds pass for all six family
parts, and Finding 1 documents (from disassembling the actual linked
firmware, not from assumption) that runtime SFR access on this core
compiles to indirect addressing through FSR1/INDF1, with an RMW trap in
that pattern that a later finding in the same document fixes. This is
exactly the kind of fact this port's device TOML and `isel-pic14e`'s
FSR emitters need to get right, and it comes from someone who already
hit the failure mode on real hardware. The revision-2 concern that this
might be aspirational, because `docs/31-ecosystem-integration-design.md`
D-1 "defers" `pic16f193x-hal`, was based on reading only epic-cc's side
of that decision: D-1 defers epic-cc's own PIC14E *compiler backend*,
not the HAL, which is exactly the gap this port closes.

---

## 7. Risks and open questions

**Most `[VERIFY]` items from earlier revisions are now resolved**, per
the confirmed citations throughout sections 1-3: the bank map, the
`MIRRORED_SFRS` set, the linear-region formula, the config word
addresses, the bit-identical-encoding claim underpinning D-1, and the
`BRA`/`BRW` encodings are all checked directly against DS41364E and,
where family-shared, cross-checked against DS40001574D. What remains
genuinely open: the exact `PCLATH<4:3>`-to-`PC` mapping for `CALL`/
`GOTO` (D-6), whether the 1939's flash/SRAM figures in DS40001574D
match what is written here for the 1937 (not yet checked directly), and
the config word *bit-field meanings* (only the addresses were checked;
the actual TOML needs the register description, DS41364E Register 4-1
onward, read in full).

**The FSR address space has three regions, confirmed exactly.**
Traditional data memory, 0x000-0xFFF (DS41364E section 3.5.1, "a
region from FSR address 0x000 to FSR address 0xFFF"); the linear
alias, 0x2000-0x29AF (D-2); and program flash, starting at 0x8000 when
the MSb of FSRnH is set (DS41364E section 3.5.3), with only the low 8
bits of each flash word readable through `INDF`, one extra instruction
cycle per access, and no write path. The simulator must model all
three and that one-extra-cycle cost. Getting this boundary wrong is a
whole class of P1 bugs, which is precisely why P1 is gated on a
byte-for-byte `gpasm` match.

**The `MOVIW`/`MOVWI` pre/post inc/dec forms.** `++INDFn`, `--INDFn`,
`INDFn++`, `INDFn-`, and the indexed `[k]INDFn` form each have a
distinct encoding (DS41364E Table 29-3, "C-Compiler Optimized" group:
`MOVIW n mm` = `00 0000 0001 0nmm`, `MOVIW k[n]` = `11 1111 0nkk kkkk`,
`MOVWI` mirrors both with the load/store direction bit flipped) and a
distinct FSR side effect. The assembler's encoding and the simulator's
FSR update must agree; this is the PIC14E analog of the PIC18
two-word-instruction risk.

**The shadow-register interrupt model, confirmed** (D-4, section 1):
W, STATUS (except TO/PD), BSR, FSR0, FSR1 and PCLATH save on entry and
restore on `RETFIE` (DS41364E section 7.5). The compiler must not rely
on ISR-side register values surviving, and the `_isr` frame copies must
be disjoint from the main frames, same as PIC18's P5. This core has no
priority levels at all, so there is no PIC18-style "does the shadow
save happen for every entry" question to ask.

**The clang side is assumed unchanged, including one question revision
1 didn't ask.** `-target msp430` remains the datalayout proxy, same as
PIC18 (8-bit `char`, 16-bit `int`). `[VERIFY]` that the PIC14E pointer
width (16-bit FSR-based, like PIC18's) does not argue for a different
proxy, and answer the `-fpack-struct`/ADR-026 question explicitly
(D-4) rather than let PIC14E inherit PIC14's `false` by omission.

**`MIRRORED_SFRS`'s PIC14E-specific set is confirmed** (section 1), but
worth repeating here as a P1/P2 boundary concern regardless: get the
implementation of it wrong and the symptom is a spurious `BANKSEL`
around FSR/BSR/WREG access, which only shows up once P2's codegen
starts exercising those registers, not in P1's hand-written `.asm`
tests. Knowing the right answer does not prevent mistyping it into
`crates/device/src/lib.rs`.

**Resolved from revision 1, kept here for visibility.** The linear
region's interaction with the allocator was revision 1's open "P3's
first design question." It is resolved in D-2 above: the allocator
never sees the linear region, and `isel-pic14e` chooses linear-vs-
banked addressing per access at emission time. If P3 finds a case D-2
did not anticipate, that is a design gap in D-2 to fix directly, not a
question to reopen from scratch.
