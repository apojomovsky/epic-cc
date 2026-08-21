# ADR-013: PIC18 interrupts (single-vector compatibility mode, MOVFF save area)

**Status:** Accepted 2026-08-20 (implemented in feat/pic18-p5-interrupts)

## Decision

PIC18 interrupt support (port P5) uses:

1. **Single-vector compatibility mode (IPEN=0) in v1.** One interrupt
   vector at `0x0008`, GIE-gated via INTCON bit 7, exactly the model
   PIC14's accepted interrupt fixtures use. The docs/29 "does the
   `__interrupt` attribute grow a priority argument" question is settled
   for v1: it does not. More than one `Func.isr` panics loudly.
2. **The vector IS the ISR body.** `.org 0x0008` + the handler's code with
   a save prologue / restore epilogue and `RETFIE`; no `GOTO` indirection
   (PIC18 `GOTO`/`CALL` are absolute 20-bit, so no paging concern unlike
   PIC14, but the entry is still the body).
3. **A 12-byte fixed save area in the Access-Bank common reservation.**
   `0x0000-0x0003` stays the retval region; `0x0004-0x000F` becomes the
   ISR save area: W, STATUS, BSR, FSR0L, FSR0H, TBLPTRL, TBLPTRH, TBLPTRU,
   plus a 4-byte snapshot of the preempted main's in-flight return value.
   The TBLPTR triplet is saved because a const read is a multi-instruction
   setup and an interrupt mid-setup leaves a torn pointer; the retval
   snapshot because an ISR that calls a value-returning function would
   clobber the preempted main's in-flight result (the exact hazard PIC14
   M13 documents and solves).
4. **MOVFF-based prologue/epilogue.** MOVFF never touches STATUS, so the
   epilogue restores STATUS and then W and preserves the interrupted
   main's Z/N, except for W's own final `MOVF 0x004,W,A` Z/N clobber (the
   one accepted flag loss, same convention as PIC14's W-last swap-back).
5. **Literal SFR access.** `inttoptr` load/store targets the physical
   12-bit address; `0x000-0x05F` (access GPR) and `0xF60-0xFFF` (SFR
   segment) both route with `a=0`, no `BSR`. `irparse`'s `inttoptr`
   address parse widened `u8` → `u16` (PIC18 SFRs are 12-bit).
6. **Shared interrupt plumbing is reused, not reimplemented.** `Func.isr`
   (msp430_intrcc), `legalize::duplicate_isr_shared` (user functions)
   and `split_isr_routines` (runtime routines) and `alloc`'s disjoint ISR
   frame region are device-agnostic from PIC14 M13 and work unchanged.

## Rationale

- **The vector/priority question is settled by the fixtures.** clang's
  `-target msp430` proxy drops the interrupt number; supporting two
  priorities would need irparse/IR/device/isel changes and two save
  areas. None of the P5 acceptance programs needs two handlers. IPEN=0
  runs the exact GIE-gated single-vector mode PIC14's fixtures exercise,
  and the INTCON bit layout is identical (bit 7 GIE, bit 4 INT0IE, bit 1
  INT0IF), so the sim gating transfers byte-for-byte.
- **The save area is what the ISR body can clobber.** W, STATUS, BSR,
  FSR0, TBLPTR, and the in-flight retval. Anything less is a silent
  miscompile class PIC14 M13 already catalogued (retval/scratch save)
  extended by the PIC18-specific torn-TBLPTR hazard.
- **MOVFF is a flag-safe copy.** The PIC14 swap-back dance exists because
  PIC14 has no flag-safe register-to-register copy; PIC18's MOVFF removes
  the need.
- **Reuse beats duplication.** The ISR marker/duplication/disjoint-region
  machinery is target-independent and already tested by the PIC14
  interrupt fixtures; P5's work is the emission and simulation half only.

## Rejected alternatives

- **Two-vector priority mode (IPEN=1) now.** Needs a priority parse (the
  attribute is dropped by clang's proxy), two save areas, two vectors
  (0x0008/0x0018), and priority-shadow semantics in the sim. No fixture
  needs it; the loud multi-ISR panic keeps the door open.
- **A 5-byte save area (W/STATUS/BSR/FSR0) without TBLPTR/retval.**
  The ISR body's const reads (TBLPTR setup is multi-instruction) and
  value-returning calls (retval live) would corrupt the preempted main.
- **Nibble-swap W/STATUS save (the PIC14 pattern).** Unnecessary on PIC18:
  MOVFF is flag-safe, so the epilogue preserves Z/N except W's own.
- **Software-pushed ISR context (compiler-synthesized PUSH/POP).**
  Hardware entry already pushes the return; only the register save needs
  the prologue.

## Revisit if

A fixture needs two handlers (add the priority plumbing and IPEN config),
or profiling shows the 12-byte save is a real cost on a small RAM part
(the save could shrink to the registers the specific ISR body actually
clobbers).
