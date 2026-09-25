# ADR-042 -- PIC18 factors repeated code on the final listing

**Status:** Accepted 2026-09-24<br>
**Decides:** `epic-cc#662`<br>
**Evidence:** `docs/44-code-factoring-spike-findings.md` (the #660 spike);
design in `docs/superpowers/specs/2026-09-24-code-factoring-pass-design.md`

## Context

Flash is the scarce resource on these parts. The spike found that the
PIC18 menu-demo listing repeats instruction runs heavily, mostly inside
single functions (inlined bodies, address arithmetic, copies), and that
sharing them as subroutines saves about 12% of flash with behaviour
checked write for write in the simulator. The technique is public
(procedural abstraction and tail merging; Fraser, Myers and Wendt 1984,
Debray et al. 2000).

## Decision

- **A new stage, `outline`,** between `isel-pic18` and `asm`: listing in,
  listing out, source locations carried line for line. PIC18 only.
- **Outline** repeated straight-line runs into leaf bodies ending in
  `RETURN`, and **merge** repeated runs that already end in `RETURN` by
  branching to one kept copy. Greedy selection by words saved, with a
  deterministic tie order.
- **Safety rules:** no branch, skip, call, return or data inside a run; no
  operand that can reach PCL, PCLATH, PCLATU, STKPTR or TOS (operands
  resolved to addresses, access-bank and bank-15 aliases included); never
  the successor of a skip, labels notwithstanding; never `FAST` forms.
- **Left alone:** inline assembly, the vector region, everything reachable
  from an interrupt vector (latency), and the runtime helpers, which are
  the inner loops of multiply, divide and float.
- **Placement:** a body whose sites sit in one small function goes right
  after that function's last unconditional terminator and is reached by
  `RCALL`/`BRA`; anything else goes after the code behind `CALL`/`GOTO`.
  `asm` relaxes an out-of-range `RCALL` to `CALL` exactly as it relaxes
  `BRA` to `GOTO`, so the reach heuristic only steers pricing.
- **Stack budget:** the pass adds one return level at most and declines
  entirely unless the main chain plus one, with the interrupt chain
  stacked on top, fits the device stack (the IR depth is the floor for
  both, since indirect calls are invisible in the listing).
- **On by default, size over cycles.** `--no-outline` opts out. The
  cycle cost is recorded rather than engineered away: the SDCC-parity
  cycle ratios were re-baselined with the pass on.

## Consequences

- Menu demo: 10992 to 9677 flash words.
- Executed instructions rise a few percent; the worst SDCC-parity program
  (malloc) costs 10.8% more cycles, math 0.6% because its helpers are
  untouched.
- A shared body serves several sites; its instructions carry the first
  site's source lines in the line table.
- A permanent differential test (`outline_differential_e2e`) compares the
  factored menu demo's layout-preserving twin against the unfactored
  program write for write in the simulator, with interrupts injected.

## Rejected alternatives

- **Skipping small loops:** measured at 40, 100 and 200 listing lines; the
  best cycle recovery (malloc to +3.1%) cost 247 words on the menu demo.
  The owner chose size.
- **Factoring in IR:** the repeats that pay are lowering artifacts, most of
  them inside one function, invisible before isel.
- **Opt-in flag first:** keeps the suite and the fuzz gate from exercising
  the pass while it is newest.
