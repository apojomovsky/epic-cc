# ADR-010: PIC18 const via TBLRD (DB-packed flash, per-byte TBLPTR re-setup)

**Status:** Accepted 2026-08-20 (implemented in feat/pic18-p4-tblrd)

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
2. **Per-byte `TBLPTR` re-setup.** Every const byte load recomputes
   `TBLPTR = table_base + k + Σ scale×%reg + byte_off` from scratch
   (`MOVLW LOW/HIGH/UPPER(table); MOVWF TBLPTRL/H/U` + carry-chain adds),
   then a single `TBLRD*` + `MOVFF TABLAT, dst`. No `TBLRD*+`
   auto-increment: that would tie multi-byte loads to ordering, the same
   hidden-state hazard P3's ADR-009 rejected for FSR0.
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
- **Per-byte re-setup is a pure function of the pointer.** Same reasoning
  as ADR-009: no hidden state, no ordering contract between the setup
  calls of a multi-byte load, and the emitter is trivially correct to
  review.

## Rejected alternatives

- **Keep the PIC14 `RETLW` chunk model on PIC18.** Computed jumps through
  `PCL`/`PCLATH` and 256-byte window alignment are a PIC14 artifact;
  PIC18 has a dedicated table-read instruction. Also every const read
  would cost a hardware-stack level (CALL/RETURN) where `TBLRD` costs
  none.
- **`TBLRD*+` auto-increment for multi-byte loads.** One setup + N
  increments is shorter, but leaves `TBLPTR` at an arbitrary place after
  the load, coupling the next access to the previous one's width. The
  per-byte model keeps every access independent, matching ADR-009's
  per-byte `FSR0` re-setup.
- **Emitting tables through `.table`/alignment directives.** Unneeded:
  PIC18's `TBLPTR` addresses flash linearly, so there are no windows to
  align to and no `.table` size assertion to enforce.

## Revisit if

A P4+ program shows the per-byte re-setup in profiling (the `TBLRD*+`
form is a drop-in, with an explicit ordering contract), or a `const`
fixture needs simultaneous indirect RAM + flash pointers beyond the
single-FSR0 + single-TBLPTR the emitters already handle.
