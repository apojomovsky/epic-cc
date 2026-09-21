# ADR-036 -- PIC18 places the local frame overlay below the globals

Status: Accepted 2026-09-20<br>
Decides: epic-cc#482

## Context

`crates/alloc` lays out two blocks of RAM: the globals, and the local
frame overlay that the call graph packs. Both packed upward from the
device's first GPR bank, globals first, on every core.

PIC18 has an access bank: RAM `0x000-0x05F` is reachable with no bank
select whatever `BSR` holds. `isel-pic18`'s `operand()` emits a `MOVLB`
for every other direct file-register access, once per straight-line run,
since the tracked `BSR` is lost at each label and across each call.

Measured on the `hal-pic18-menu-demo-18f4550` size fixture: 817 `MOVLB`
instructions, **791 of them selecting the bank the frames happened to
land in** (`0x21F-0x2DE`, above about 500 bytes of globals). Globals
barely use the window they were occupying: they mostly move through
`MOVFF` and `FSR`, which carry a full 12-bit address and never touch
`BSR` at all.

The window is 80 bytes (`gpr_start()` `0x010` to `access_bank_hi`
`0x05F`, with `fixed_retval` holding `0x000-0x00F`), and the menu-demo
overlay is 193 bytes, so the window cannot hold every frame. It holds
most of them: 100 of the fixture's 126 frames.

## Decision

On a device that declares `access_bank` (PIC18), place the frame overlay
at the GPR start and the globals immediately above it. PIC14 keeps the
historical order: it has no access bank, and its analogue, common RAM, is
carved out of every bank already.

- The frame overlay's shape (slot widths, call graph, topological order,
  chain depth) never depended on where the globals land, so those steps
  run first and the base assignment becomes one operation over a region
  start, called before the globals on PIC18 and after them on PIC14.
- A global pinned by address (`__at`) inside the span the overlay wants
  has nowhere to go, so those modules keep the globals-first layout.
- A module whose globals no longer fit in the shorter window above the
  overlay also falls back to globals-first rather than failing to
  compile. A density win never justifies rejecting a program.
- Which frames get the window is still decided by call-depth stacking,
  not by how busy each frame is. Choosing deliberately is epic-cc#501.

## Consequences

`hal-pic18-menu-demo-18f4550`: 817 `MOVLB` to 421, 14726 to 14280 flash
words (-3.0 percent), 736 to 739 RAM bytes. The three extra RAM bytes are
the globals' own even-alignment and a pinned global's bump landing
differently once their cursor starts above the overlay; no byte of demand
moved, only its order.

Frames-first can only lower a frame's base, so `isel-pic18`'s assertion
that a float runtime routine's frame fits the access-bank GPR region is
strictly easier to satisfy after this change, never harder. That was the
problem `docs/36` set out to solve by reserving the window and pushing
globals to `0x60`; its premise is now stale and it is annotated as such.

The layout change is observable to any test that derives a global's
address itself instead of reading the compiler's `--map`: a difference as
far upstream as switch expansion now moves every global address. Three
harnesses did exactly that and were converted; epic-cc#503 tracks the
rest.

## Rejected alternatives

- **Slot-granular preference** (the original epic-cc#482 sketch: hot GEP
  index slots, memcpy pointer slots, high-reference narrow locals). A
  `MOVLB` dies only when *every* slot a run touches is in the window, so
  moving individual slots leaves mixed runs that re-bank anyway. Measured
  at -15 words on this fixture by an earlier pass.
- **Reserving the window** for a slot category, as `docs/36` proposes.
  Costs the reservation's bytes outright when the overlay does not fill
  it, and an earlier pass measured +118 RAM bytes for the reservation
  plus the displaced globals' tail.
- **Conflict-based first-fit ordered by reference density.** Sound
  (co-liveness is exactly ancestor/descendant in the call graph plus the
  main-versus-ISR split) but it fragments: cold deep frames placed late
  land high and push their large ancestors higher. 269-byte overlay
  against the stacking's 193.
- **Mirroring the overlay inside its own span**, which preserves both
  span and disjointness. Measured worse, 459 against 419, because it
  evicts the frames the stacking already had in the window.

## Revisit if

A PIC18 part ships with a wider access bank, or the window's occupants
stop being chosen by call depth (epic-cc#501), at which point the
trade-off between the two blocks is worth re-measuring rather than fixed
per core.
