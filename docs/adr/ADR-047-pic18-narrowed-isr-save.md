# ADR-047 -- PIC18 ISR context save narrows to the reachable clobber set

**Status:** Accepted 2026-10-08<br>
**Decides:** `epic-cc#783`<br>
**Evidence:** control-demo listing before/after (`PIC18_IRQ_Handler` 35
save `MOVFF`s down to 19, flash 9519 down to 9487 words); the size ladder
(menu `-O2` -64, pid -62, bridge -7, control -32, menu `-Os` +2 positional
churn, see below); `isr_narrow_e2e` plus the existing ISR sim gates
(`prod_isr`, `tablat_isr`, `fsr1_isr`, `float_isr`, `interrupt_mul`)

## Context

Every PIC18 ISR paid the full context save: 18 prologue `MOVFF`s plus W
(and the mirrored epilogue) covering STATUS, BSR, FSR0/1, PROD, TABLAT,
TBLPTR, PCLATH/PCLATU, and the retval backup, whatever the handler
touched. The dispatch these handlers wrap rarely multiplies,
table-reads, or switches, so PROD, TBLPTR, TABLAT, and FSR1 bytes
shuttled dead state on every interrupt. `epic-cc#641` showed the other
direction failing: the set once omitted PCLATH/PCLATU and a Timer2 IRQ
inside a main-line switch window mis-dispatched. Narrowing must therefore
stay complete by construction, never by review.

## Decision

- **Narrow per ISR from the emitted text, not the IR.** After pass A
  buffers every body, `isel-pic18` walks each ISR's `CALL` edges through
  ordinary, runtime-routine, naked, and module-asm bodies and records
  which save classes (W, STATUS, BSR, FSR0/1, PROD, TABLAT, TBLPTR,
  PCLATH/PCLATU, retval) the reachable instructions can write. Scanning
  the final text keeps the model honest: a lowering the scan does not
  understand keeps the full set instead of miscompiling.
- **Drop only exact save lines.** The filter removes lines identical to
  the prologue/epilogue emission for unneeded classes and refuses loudly
  on any count mismatch (prologue once per body, epilogue once per
  `ret`), so emission drift is a compile error, not an under-save.
  Unknown call targets, unrecognized mnemonics, and suffixless naked
  stores all keep the full set.
- **Shared save stub rejected.** One save subroutine would still need
  per-entry W handling and only pays off with several handlers; every
  program in the ladder has one ISR, so the shared stub saves nothing
  where narrowing saves 32 to 64 words.
- **Two accepted residuals.** The `sfr-context-save` density bucket does
  not move: it counts FSR-seed/`INDF` staging triples inside ordinary
  functions (pointer walks, flash-string copy loops), a different lever
  filed separately. Menu-demo `-Os` grows 2 words while its listing
  loses 16 lines: the deletion shifts downstream code 64 bytes, and the
  assembler's `.pclalign` NOP pad for a PCL-table block re-settles 34
  words heavier ending exactly on its page boundary (proven by
  HEX-diffing both images). Positional churn, no new codegen; net across
  the moved rows is -163 words.

## Revisit if

A second ISR per program makes the shared stub worth pricing, or a
handler appears whose reachable set the text scan cannot model without
keeping everything (then the scan, not the save set, is the bottleneck).
