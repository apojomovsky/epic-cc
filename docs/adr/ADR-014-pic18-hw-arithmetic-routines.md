# ADR-014: PIC18 arithmetic routines (hardware MULWF, branch-based divmod)

**Status:** Accepted 2026-08-20 (implemented in feat/pic18-p6-long-mul)

## Decision

PIC18 32-bit `long` and hardware-multiply support (port P6):

1. **Legalize is untouched and device-agnostic.** Every `mul`/`udiv`/
   `urem`/`sdiv`/`srem` and variable-count `shl`/`lshr`/`ashr` (i8/i16/i32)
   lowers to a runtime `Inst::Call` with the injected routine `Func`
   (params + `__scr` alloca), exactly as on PIC14. The routine names and
   `__scr` sizes are legalize's contract.
2. **The recipes live in `isel-pic18`, not `isel`.** PIC18's runtime
   routines are rewritten for the instruction set:
   - **Hardware `MULWF` schoolbook mul for all widths.** u8 = one MULWF;
     u16 = 3 partials (P11 at shift 16 dropped); u32 = 10 partials with
     `i+j < 4` (the rest land past bit 32 and are dropped). No shift-add
     loop exists on PIC18. The P6 headline from docs/29 §1's mul row.
   - **Branch-based restoring division.** The PIC14 skip-sensitive
     `BTFSS`/`INCFSZ` borrow folds become real `BNC`/`BRA` branches and
     `SUBFWB` (f - W - !C), the exact PIC18 instruction that expresses
     the borrow chain. **No single-GPR-bank constraint**: PIC18 branches
     are absolute, so a `MOVLB` between a test and its target is
     harmless, and `alloc`'s routine-base rounding already no-ops on the
     single contiguous PIC18 region.
   - **`RLCF`/`RRCF` shifts** (through-carry rotates, the PIC18 names of
     `RLF`/`RRF`), and the Z-chain negate (COMF all, INCF low, `BTFSC
     STATUS,2` before each higher `INCF`).
   - **`_isr` copies** share the recipe body against their OWN slots (the
     `cur_func` map), so the ISR frame never overlaps the main frame.
3. **The u8 remainder is 2 bytes** ("the 8-bit rem shift can carry"),
   the PIC14 layout contract kept; the divisor's implicit high byte is
   folded with `MOVLW 0` + `SUBFWB`/`ADDWFC`.

## Rationale

- **Legalize's contract is right; only emission differs.** The routine
  names, params, `__scr` sizes, and retval-region result placement are
  target-independent. A device hook in legalize (a `has_hardware_multiply`
  gate) would be a second copy of the name table for no benefit: the
  recipe emitter sees the call either way.
- **MULWF is single-cycle and exactly 8x8.** Schoolbook partial products
  are the smallest correct mul; the PIC14 shift-add loop (16/32
  iterations) is dead weight on a core with a multiply instruction.
- **Branch-based loops are simpler and the sim already models them.**
  `BNC`/`BZ`/`BN` etc. are in `exec_cond_branch`; `SUBFWB` in the byte
  arm. The recipes are the same algorithms PIC14 verified (epicurus
  heritage), ported instruction-for-semantics.
- **The single-bank constraint is a PIC14 skip artifact.** The PIC14
  routines' frames must fit one GPR bank because a `BANKSEL` between a
  skip-test and its target changes the skip. PIC18 branches have no such
  hazard, so the constraint and `alloc`'s rounding die together.

## Rejected alternatives

- **Port PIC14's shift-add mul loops and skip-sensitive bodies verbatim.**
  Works, but wastes the headline hardware multiply and carries the
  single-bank constraint for nothing.
- **A `Device::has_hardware_multiply` gate in legalize.** Legalize is
  device-agnostic by design; the gate would thread `Device` through it
  for a capability only the backend emission cares about.
- **Software mul via the PIC14 recipes on PIC18.** Same correctness, more
  flash and cycles; contradicts docs/29 §1.

## Revisit if

A part with a different multiply story appears (none: all PIC18 cores
have MULWF), or profiling shows the 4-partial u32 mul's repeated `MOVF`
loads cost more than a shifted single-multiply chain (a
`MULLW`+`MULWF` two-register trick exists, but it needs W management
that the schoolbook form avoids).
