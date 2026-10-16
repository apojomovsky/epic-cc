# ADR-050 -- PIC18 refuses pointer phi cycles that need a copy temp

**Status:** Accepted 2026-10-11<br>
**Decides:** `epic-cc#1117`<br>
**Follow-up:** `epic-cc#1127` (iselcore seeder plus 2-byte temp for pointer phi copies)<br>
**Evidence:** `crates/driver/tests/flash_walk_e2e.rs`
(`flash_walk_phi_swap_is_refused_on_p18`),
`crates/driver/tests/fixtures/flash_walk_swap.c`

## Context

Two `const char *` walk phis swap each iteration, one advancing through
a gep over the other. The back-edge copies `a <- b` and `b <- gep(a, 1)`
form a cycle: the gep over a phi cannot resolve while the phi's arms wait
on it. `iselcore` stalls and panics with `cyclic or unresolvable pointer
chain`, listing the pending geps and selects.

Lifting the refusal takes two pieces. An optimistic seeder in `iselcore`
(greatest fixpoint over the unresolved pointer phis) breaks the
resolution cycle. Seeding alone compiles the swap to wrong code. With the
seeder in place, `isel-pic18` `emit_phi_copies` sets the copy source to
`None` for resolved arms, adds no ordering edges, and emits the copies in
index order, so the second copy reads an already-overwritten `a`. The
simulator gave `sum` = 149 against a hand trace of 162. The backend also
needs a temp for the copy cycle.

## Decision

- Pointer phi cycles that need a copy temp stay refused, with the
  existing `iselcore` message.
- The refusal is pinned by `flash_walk_phi_swap_is_refused_on_p18`, run
  on the swap fixture. The fixture keeps a volatile trip count, so the
  loop is not folded away.
- Support is tracked in `epic-cc#1127`: the optimistic seeder in
  `iselcore` together with a 2-byte temp in `emit_phi_copies`, where
  resolved `Slot` arms record the slot they read as their copy source.

## Rationale

A refusal is correct and visible. Compiling the swap wrongly is a silent
miscompile, which the project rejects (panics are the error surface).
The temp location is an allocator reservation trade-off, either a fixed
scratch or PROD, and it needs a decision that this change does not make.

## Rejected

- **Seeder without a backend temp.** Compiles to wrong values (149
  against 162).
- **Changing the expected values to match the output.** Hides the
  miscompile.
- **A new refusal message.** The existing message already names the
  failing chain, and changing it would be an interface change for no
  gain.

## Revisit when

`epic-cc#1127` lands, with both the `iselcore` seeder and the backend
temp. The refusal test then becomes a value test: `sum` = 162 and
`out` = 68 (`D`).
