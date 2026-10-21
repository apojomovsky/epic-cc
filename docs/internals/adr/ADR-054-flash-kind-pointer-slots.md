# ADR-054 -- PIC18 pointer slots carry a static flash or RAM address space

**Status:** Accepted 2026-10-14 (oracle decision on `epic-cc#1119`;
amended 2026-10-17 by `epic-cc#1306`; amended 2026-10-21 by
`epic-cc#1346`)<br>
**Decides:** `epic-cc#1119`, `epic-cc#1092`, `epic-cc#1306`,
`epic-cc#1346`<br>
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
  source stay RAM. Indirect stores, memcpy destinations, and asm
  outputs write their resolved global precisely; unknown-target writes
  demote taken globals (globals taken as values, the only ones an
  unknown pointer can alias). Globals initialized with a flash address
  start flash-kind.
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
  Maybe-flash values are refused on the same value paths (stores,
  call args, returns). Integer-laundered addresses (loaded as data,
  converted back) stay out of scope in `epic-cc#1284`.

- The PIC18 driver predefines `__EPIC_CC_FLASH_PARAMS__` so sources can
  gate away the RAM-staging workaround for const literals while still
  building with releases that predate this ADR.

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

## Runtime-tagged fallback (epic-cc#1306)

Static spaces stay the default: a pointer proven to one space keeps its
2-byte slot with no per-deref branch. Only maybe-flash slots get the
3-byte tagged form (tag plus 16-bit address, tag 0 for RAM and 1 for
flash), so mixed cases refused before now compile on PIC18 only.
Call results used as pointers and freezes join the slot proof, and a
function returning maybe-flash on any path writes every pointer
return fat, so no RAM return beside a mixed one leaves a stale tag.
The tag is set at every join: stores into shared global slots
(direct or folded, value plus tag), phi incoming edges, select arms,
freezes, call arguments into mixed params, and mixed global
initializers (flash when the initializer names a table). Dereference,
store-through (whose flash arm traps: a ROM write has no lowering),
memcpy source, and call argument and return paths dispatch on the tag
with identical result bytes either way.
A directly proven flash value into a mixed slot keeps its refusal on
the store, call argument, return, and freeze paths: the fallback
serves join values and initializers, not a statically known ROM store.
Proven single-space pointers change neither size nor code: the size
ladder measures byte-identical branch versus master on every bench,
and benches carry no mixed pointer to widen. Deferred, each filed as
an issue under epic-cc#1306: int-laundered addresses (no tag survives
`ptrtoint`), PIC14 (no `TBLRD` path there), and memcpy into a
maybe-flash destination (a raw byte copy has no tag source).

## Integer-domain space proof (epic-cc#1346)

An address that passes through the integer domain keeps its space when
the space is settled statically, so no runtime tag is needed.

The key fact is what an integer read of a pointer slot holds: the
slot's own bytes, which are the address, not the table content. A
flash-kind slot read as `i16` yields the table's byte address, and
the `inttoptr` of that integer is the same address a direct pointer
would carry. The read of a *const table* yields the table's content,
whose first two bytes are the address of the const it names.

`iselcore::add_int_flash_definite` proves the space with a greatest
fixpoint by removal, the mirror image of the maybe-taint: a value is
definite only when every path that produces it yields a flash address,
and the state only shrinks, so the iteration terminates. A value is
definite when it is a flash const global, a two-byte load from a
location whose two bytes are definite, a copy of a definite value, an
add of a definite value and a constant (a subtraction keeps the space
only when the address is the minuend, and a difference of two
addresses is a length, a mask or a shift leaves a fragment), or a
select, phi, or call result whose every arm is definite.

Memory bytes are definite when every write to them writes a definite
value, so a slot that also takes RAM loses the proof for exactly those
bytes. A write must also have happened by every read of the byte: a
read before a later write observes the earlier value, so the proof
tracks the block and the position of each write and each read, and a
write in another block counts only when that block dominates the
read's. Without that order, a global read before its only store would
observe the store's value.

A pointer that joins a definite flash pointer with a value that is
not a flash address is maybe-flash, and no tag survives the join, so
the proof withdraws the flash claim from the join's result and marks
it mixed. The join's own dereference then keeps the maybe-flash
refusal; the flash arm it joined keeps its claim, so a direct
dereference through the arm still reads flash. The withdrawal is what
keeps the maybe case safe without the 3-byte tagged form: a
maybe-flash slot is sized by `alloc` from its own slot proof, so
marking the `inttoptr` dst a flash writer there would widen the slot
on the backend side only.

The proof leaves the `inttoptr` dsts out of the slot proof's writer
set, so the set is extended in the backend alone, for the deref path
only.

Six shapes stay refused. A `ptrtoint` of a GEP reads the GEP's own
slot, which `resolve_pointers` folds away and never writes, so the
value is not an address. A store of a flash address through a pointer
the proof cannot resolve names no location, so no byte is definite. A
`memcpy` side with a runtime term names no fixed bytes, so the copy's
other side is not proven to hold the address. A call result is
definite only when the callee returned a parameter it never
overwrote, since the IR is a slot IR and a name may be reused. A
global whose initializer holds a plain value holds no address, so its
bytes are not definite. A subtraction with a constant minuend is not
an address, so it does not keep the space.
