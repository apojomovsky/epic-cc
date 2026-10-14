# ADR-053 -- PIC18 pointer slots carry a static flash or RAM address space

**Status:** Accepted 2026-10-14 (oracle decision on `epic-cc#1119`)<br>
**Decides:** `epic-cc#1119`, `epic-cc#1092`<br>
**Evidence:** `crates/isel-pic18/tests/flash_slots.rs`,
`crates/iselcore/tests/slot_spaces.rs`,
`crates/driver/tests/flash_walk_e2e.rs`
(`flash_arg_walk_reads_program_memory_on_p18`, mixed-phi RAM-copy tests)

## Context

A flash address stored in a RAM pointer slot (`g = "..." + n` with `g`
a RAM pointer global) has no lowering: a later load reads data memory
at a flash address, silently wrong. The compiler refused it
(`epic-cc#1101`). The same gap blocked the flash-read path for const
pointers reaching a pointer phi (`epic-cc#1092`): the menu demo's
`put_str` walks `s[len]`, LLVM LSR turns that into a pointer phi, and
the phi materializes in a RAM slot, so reading it through TBLPTR is the
same question as storing a flash address into a RAM slot.

## Decision

- Every pointer slot has a static address space, proven whole-program
  (`iselcore::flash_slot_spaces`): params and phi/select/gep/load-ptr
  dsts keyed `{func}::{slot}`, globals as `@{global}`. A slot is
  flash-kind when every address it can hold is a flash table address
  (null alone does not qualify), computed as a greatest fixpoint over
  writers plus an ungrounded prune, so forwarding cycles with no const
  source stay RAM.
- Loads through flash-kind slots seed `TBLPTR` from the slot bytes
  (`emit_flash_tblptr_seed` plus dynamic terms); stores of flash
  addresses into flash-kind globals materialize whole (`LOW`/`HIGH`,
  or a `TBLPTR` seed for dynamic offsets). No tag bit, no per-deref
  branch.
- Mixed slots keep RAM copies: `alloc` skips the copy only for consts
  whose every use is flash-proven (`flash_resident_consts`
  fixpoint, PIC18 only), decided with the same predicate `isel` reads
  from the final map, so both proofs agree. Uses behind GEP chains and
  select/phi arms are followed (`iselcore::const_fan_in`); copies
  follow derivations while pool and staging bookkeeping stay
  syntactic.
- The `isel` refusals stay as the safe fallback for anything unproven:
  flash values into RAM slots, through-ROM writes, flash returns, and
  flash arguments to RAM params (keeping the over-255 user error).

## Rationale

A runtime tag charges every pointer dereference about 6 words plus a
branch, even in pure-RAM programs, on a core where `TBLPTRU` is fixed
at zero. Always copying to RAM spends scarce RAM, and the reverted
staging pass measured byte-identical. The static proof adds no cost
per dereference, reuses the existing provenance and staging code, and
fails into today's refusal, which is safe. It also fixed two silent
miscompiles the work exposed: a phi of flash consts forwarded to a
callee read data memory at the flash address, and a walk value stored
into a RAM global did the same.

## Rejected

- **Tagged pointer slots.** Per-deref branch and an extra RAM byte per
  slot, paid by programs that never touch flash.
- **Always RAM-copy escaping consts.** RAM cost per literal with no
  flash saving; the staging measurements showed no win.
- **Keeping the refusal.** Correct but rejects working programs: the
  menu-demo shape and the global-slot store shape both compile now.
