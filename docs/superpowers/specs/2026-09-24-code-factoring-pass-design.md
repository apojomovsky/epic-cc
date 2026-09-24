# PIC18 code-factoring pass (design)

Date: 2026-09-24. Ticket: epic-cc#662. Evidence: `docs/44` (the #660
spike), which measured 1487 flash words saved on the menu demo
(10992 to 9505) with behaviour checked write for write in the simulator.

## Goal

Factor repeated instruction sequences out of the final PIC18 listing:
outline straight-line repeats into shared leaf bodies, and merge
repeated tails that end in `RETURN`. Size is the product's scarce
resource; the price is a few percent more executed instructions and at
most one extra stack level.

## Where it runs

A new crate, `outline`, between `isel-pic18` and `asm`, in the PIC18 arm
of the driver where the listing currently passes straight through:

```
isel-pic18 -> outline -> asm
```

Listing in, listing out, `locs` carried line for line (the
`peephole::optimize_with_locs` contract), so `--emit asm` shows the
factored program and a miscompile still bisects to a stage. PIC18
only: PIC14's 8-level stack and `PCLATH` paging need their own pricing
and are out of scope.

**On by default** for PIC18, with `--no-outline` to opt out (timing-
critical code, or stepping through inline copies in the debugger).
Default-on is what makes every existing PIC18 e2e test and the fuzz
gate exercise the pass from day one. Size pins only fail on growth, so
no baseline churn is forced; refreshing `size_baseline.toml` lands in
the same PR to lock the gain in.

## Algorithm

1. **Parse** the listing into instructions, labels and directives.
   Operands resolve to addresses first: `equ` symbols expand, and
   access-bank operands `0x60-0xFF` alias `0xF60-0xFFF`.
2. **Eligible instruction:** not a branch, skip, call, return, `SLEEP`,
   `RESET`, `PUSH`/`POP` or data; touches none of PCL, PCLATH, PCLATU,
   STKPTR, TOSL/H/U.
3. **Excluded regions:** inline-assembly regions (`; --- asm start/end
   ---`), naked functions, the vector region, and every function
   reachable from an interrupt vector (latency). Roots are the code at
   `org 0x0008`/`0x0018`, or the `GOTO` target of a vector stub in
   priority mode.
4. **Candidates:** runs of 2 or more eligible instructions inside one
   basic block (labels and control flow split blocks), up to a length
   cap, whose first instruction is not the textual successor of a skip,
   labels notwithstanding. A run may end in `RETURN`: that is a tail
   candidate.
5. **Selection:** greedy by gain, `k*W - (k*c + W + 1)` for outlines,
   `(k-1)*(W - j)` for tails, over non-overlapping sites, with a total
   order on ties (gain, then first site position) so output is
   deterministic.
6. **Placement:** a body whose sites all sit in one function no larger
   than the reach budget goes right after that function's last
   unconditional `RETURN`/`BRA`/`GOTO`, and its sites use `RCALL`/`BRA`.
   Never where an `org` follows before the next instruction. Everything
   else goes before `end`, reached by `CALL`/`GOTO`. Tail merges keep
   the first copy in place under a new label.
7. **Names and locs:** bodies are `__pa<N>` in selection order. Body
   lines carry the first site's locs, a replaced site carries its first
   instruction's loc, so the line table points into a real source line.

`CALL`/`RCALL` and `RETURN` preserve W, STATUS and BSR, so a body runs
under its caller's state exactly as the inline copy did. The pass never
emits `FAST` forms.

## Safety nets

- **Assembler:** out-of-range `RCALL` relaxes to `CALL`, the same
  fixpoint rule that already turns a far `BRA` into `GOTO`. A reach
  heuristic that misjudges costs a word, never correctness.
- **Depth:** the pass recomputes the return-stack depth on its own
  output (main chain plus the interrupt chain stacked on top, helpers
  included) and panics if it exceeds the device stack. The IR-level
  check cannot see listing-only calls.
- **Idempotence:** running the pass on its own output selects nothing
  new of positive gain; a unit test pins it.

## Tests

- Unit (crate): each eligibility and exclusion rule on micro-listings,
  skip successors across labels, placement next to `org`, tie-order
  determinism, idempotence.
- **Differential e2e** (the spike's method): compile the menu demo
  with and without the pass, build the layout-preserving twin (sites
  padded with `NOP`s, bodies at the end), and compare the ordered RAM
  write stream in the simulator, to halt and with interrupts injected.
  Plus small programs aimed at tail merges and skip successors.
- The existing PIC18 e2e suite and the fuzz gate run through the pass
  by default.
- `size_baseline.toml` refreshed to the new numbers.

## Rejected alternatives

- **Factoring in IR (outlining functions before isel):** repeats that
  pay off are mostly address arithmetic and copies that only exist
  after lowering; the spike found 80% of the gain inside single
  functions at instruction level.
- **Opt-in flag first:** keeps the suite from exercising the pass
  while it is newest, the moment coverage matters most.
- **Suffix-tree optimal selection:** more code for an unmeasured gain
  over greedy; the selector can be swapped later behind the same
  interface.
