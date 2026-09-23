# ADR-041 -- PIC18 carries provable callee exit banks across calls

**Status:** Accepted 2026-09-22<br>
**Decides:** `epic-cc#495`<br>
**Evidence:** `docs/42-density-ceiling-findings.md` (Experiment 1), Task 0
gate measurement on this branch (20 provable-exit CALL sites, 236 MOVLBs
before, 184 after on the menu-demo listing)

## Context

PIC18 banks RAM through BSR, and `isel-pic18` tracks the selected bank per
block so a `MOVLB` is emitted only when the needed bank differs from the
tracked one. Every `CALL` return cleared that tracked bank unconditionally:
the callee ran its own bank sequence, so the caller could not know what it
left live. `docs/42`'s Experiment 1 measured the cost on the menu-demo
listing: a BSR dataflow with calls clobbering finds the large majority of
`MOVLB`s provably redundant under exactly that call rule. The redundancy is
not free money, it is the price of the rule; the question was whether the
per-callee exit bank can be known cheaply and truthfully.

It can: `isel-pic18` already emits each function into its own `Gen` buffer
and records every block's end bank. A function whose RETURN-ending blocks
all agree on one end bank has a provable exit bank, and every caller may
keep that bank live across the call.

## Decision

- **Buffered reverse-topological emission.** Functions emit into
  per-function buffers, callees before callers (call-graph order, recipes
  and naked bodies excluded), and the buffers concatenate in the original
  module order, so the output stream keeps master's layout apart from
  `tmp{n}` label renumbering.
- **Exit-bank map from recorded return ends.** After each function's `Gen`
  run, the recorded end states of its RETURN-ending blocks join by
  unanimity: all known and equal gives the bank, any dirty, missing, or
  disagreeing end gives unknown.
- **Direct and indirect call arms carry.** The direct `CALL` arm and each
  indirect-call candidate arm set the tracked bank from the map instead of
  clearing; the indirect done label restores the candidate meet directly,
  because its linear fall-through comes from the trap block, whose bank is
  unknown.
- **Unknown clears.** An absent map entry (runtime recipe, naked body,
  forward reference outside the graph) or an unknown exit clears the
  tracked bank exactly as master did.

## Rationale

**Emitter-truth.** The map is derived from the same emission run's recorded
end states, so it cannot disagree with the emitted text. A separately
computed dataflow (the Experiment 1 analyzer) can drift from what the
emitter actually does; this map cannot, because it is what the emitter
recorded while writing the listing.

**No return-site tax.** A restore-on-return convention would make every
callee restore a contract bank before returning, taxing every call site
with instructions it may not need, to save selects at some of them. Carry
costs nothing at sites whose callee exit is unknown (they clear, as today)
and nothing at callees; it only removes instructions.

Measured on the menu-demo fixture (whole-program, 18F4550): function-body
`MOVLB`s drop 236 to 184, and the size ladder improves with no row growing
(bench-switch 94 to 64 words, pic16 encoder 7071 to 6984, menu-demo 12137
to 11974). The simulator is the oracle for every elided select in the unit
tests, which pin carry across direct and indirect calls, conservatism on
poisoned and disagreeing exits, forward-defined callees, and the recipe
fallback.

## Consequences

- Calls to runtime recipes keep the post-call `MOVLB`: recipes have no
  `Gen` run and no map entry. The residual provable-exit sites in the
  post-change listing are exactly the `__mul_u32`/`__mul_u16` callers.
  Giving recipes recorded exit banks is deferred, not decided here.
- Callees with valued returns or terminator-selected banks (per-edge phi
  copies) poison their end state and stay unknown. Expected and
  conservative.
- The call graph must be a DAG for the ordering to be total (recursion is
  a compile error upstream); a defensive cycle break degrades to module
  order and an absent map entry.

## Rejected alternatives

- **Restore-on-return convention (callee restores a contract bank).**
  Rejected on `docs/42`'s measurement: it taxes every return path with
  restore instructions to save selects at only some call sites, and it
  couples every callee to a global convention instead of its own measured
  exit state.
- **Port the PIC14 banking pass to PIC18.** Rejected as a duplicate
  tracking system: a separate pass would re-derive bank state the emitter
  already knows, and the two derivations could disagree. The exit-bank map
  keeps one owner of the truth: the `Gen` that emitted the text.
