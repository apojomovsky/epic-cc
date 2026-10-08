# ADR-010: PIC18 const via TBLRD (DB-packed flash, per-byte TBLPTR re-setup)

**Status:** Accepted 2026-08-20 (implemented in feat/pic18-p4-tblrd).
Walk amendment 2026-10-08 (epic-cc#745, epic-cc#778): adjacent reads share
one seed and walk `TBLRD*+`, superseding item 2 below.

## Decision

PIC18 `const` (flash) globals are read with the `TBLRD` family
(`TBLRD*`/`*+`/`*-`/`+*`), using:

1. **A linear flash model, no chunking.** Program memory is byte-packed
   (two bytes per 16-bit word, little-endian: even byte = word low, odd
   byte = word high), exactly `gpasm -p p18f4550`'s `DB` packing. The
   tables are emitted as `DB` lines after `__start`; `LOW`/`HIGH`/`UPPER`
   of the table label resolve the byte address (the assembler's PIC18
   symbol table is byte-addressed, P1's proven convention). The 511-byte
   `RETLW` ceiling of PIC14 stops existing: any table that fits flash is
   linear.
2. **Shared seeds with `TBLRD*+` walks.** Adjacent const reads seed once
   and walk: static-adjacent reads share via the backend tracker
   (epic-cc#745), and dynamically indexed runs share when a pre-scan
   proves same table, identical terms, and consecutive offsets with no
   term-slot write or `TBLPTR` use between (epic-cc#778). The ordering
   contract that answers the old hidden-state objection: every walk
   advances exactly one byte per read, labels and calls end all runs,
   and the ISR prologue saves `TBLPTR`/`TABLAT`, so a walked sequence
   survives interrupts like the memcpy walk does.
3. **Loud ROM-write panic.** A `store` through a `const` base panics
   ("ROM is not writable"), matching PIC14's store-through-const panic.
4. **Dynamic index adds onto `TBLPTR` with full 21-bit carry**, and a
   16-bit index register contributes both bytes (its high byte adds onto
   `TBLPTRH` with carry into `TBLPTRU`), so `table[0x1XX]` reads the right
   byte.

## Rationale

- **PIC18's ISA gives this for free.** `TBLRD` is a single-word opcode;
  `TBLPTR` is a 21-bit byte address with no window or page constraints.
  PIC14's `RETLW` machinery (computed-goto `PCL` jumps, 256-byte
  `PCLATH` windows, chunk chaining, `PAGE`/`LOW`/`HIGH` restore maps) is
  entirely dead weight on PIC18 and is not ported: the tables are plain
  data, and a read is a 3-instruction setup + `TBLRD*` + copy.
- **`DB` is the assembler's native byte form** and `gpasm` packs it the
  same way our assembler does (verified byte-for-byte), so the HEX
  cross-check oracle holds without special-casing.
- **Sharing is a proven adjacency, not trusted state.** Same reasoning
  as ADR-009's reuse rules: a skip happens only when the previous site
  provably left `TBLPTR` on the wanted byte (identical table and terms,
  consecutive offset, no term-slot write between), and every label, call,
  or second table ends the run. The emitter stays reviewable because
  each skip cites that proof instead of a standing belief.

## Rejected alternatives

- **Keep the PIC14 `RETLW` chunk model on PIC18.** Computed jumps through
  `PCL`/`PCLATH` and 256-byte window alignment are a PIC14 artifact;
  PIC18 has a dedicated table-read instruction. Also every const read
  would cost a hardware-stack level (CALL/RETURN) where `TBLRD` costs
  none.
- **`TBLRD*+` auto-increment for multi-byte loads.** Adopted (see item 2):
  one setup plus N walks, with the ordering contract the original entry
  asked for. Isolated reads keep the per-byte model, so every access
  stays independent, matching ADR-009's per-byte `FSR0` re-setup.
- **Emitting tables through `.table`/alignment directives.** Unneeded:
  PIC18's `TBLPTR` addresses flash linearly, so there are no windows to
  align to and no `.table` size assertion to enforce.

## Revisit if

A `const` fixture needs simultaneous indirect RAM + flash pointers
beyond the single-FSR0 + single-TBLPTR the emitters already handle.
(The profiling clause fired as epic-cc#745 and epic-cc#778; loop-carried
sharing across back edges is tracked as epic-cc#961.)
