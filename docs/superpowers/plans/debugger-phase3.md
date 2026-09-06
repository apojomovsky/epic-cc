# Debugger phase 3: sim control surface

**Ticket body (to become `epic-cc#NNN`)**. Mirror `#246`'s structure.

---

## Summary

Third phase of the source-level debugger (`docs/34-debugger-design.md`
§4; umbrella `#203`). Phase 1 gave the address-to-line table; phase 2
gives the typed variable table. This phase gives `crates/sim` the
control surface the phase-4 gdbstub drives: run-until-address,
register read/write, memory read/write, and instruction-granular
stepping. Scope for v1 (settled 2026-09-06): **PIC14 core only**
(`Pic14` on the F877A), proving the pattern per `docs/34` §5 non-goal
"PIC18. Land the PIC14 path first."

Depends on: none beyond phase 1. This phase does **not** depend on
phase 2's type table: `print <name>` and `info address` are phase-4 RSP
behaviors built on that table; phase 3's own scope (run/step/registers/
memory) consumes no type data. **Area conflict with pic14e P1
(`#246`), which is claimed and edits the same `crates/sim` crate**: the
two cannot be worked simultaneously (AGENTS.md area rule), so this
ticket's board tooling must serialize it against `#246`; it is not
"Blocked by: none" in the scheduling sense. Umbrella: `#203`.

## Scope

- **`run_until` primitive.** Run instructions until `pc` reaches a
  target address, the program halts, or a step cap is hit. Returns the
  stop reason (reached / halted / capped). This is the breakpoint
  mechanism: the gdbstub sets a watch on an address, the sim runs
  until it lands there. No in-sim breakpoint table; the adapter (phase
  4) owns the set.
- **Register surface.** Reads of `W`, `PC`, `STATUS` (with the active
  bank derived from STATUS `RP1:RP0` per `bank_base()`), `FSR`
  (`0x04`), `PCLATH`, `INTCON`, and writes to `W`, `PC`, and the bank
  select bits. The sim already keeps `pc`, `w`, `ram` fields and a
  `bank_base()`; this phase adds a coherent public accessor set rather
  than raw field pokes.
- **Memory read/write.** Uniform `read_mem(addr, len)` /
  `write_mem(addr, bytes)` over the RAM image (`ram`/`ram_mut`),
  banked-resolved the way `banked_addr` resolves direct operands, plus
  read access to program flash (`prog`) since DWARF const arrays and
  the PC's instruction stream live there.
- **Step with stop semantics.** `step` already advances one
  instruction at a time; phase 3 adds the control-surface wrapper (one
  explicit step or a bounded run) the adapter drives. `sim` remains
  strictly instruction-granular: it has no notion of a source line.
  **Line-granular stepping (stop on the first instruction whose address
  is on a different source line, including the phase-1 `BANKSEL`
  inherits-a-line case) is phase 4's job**, done by the adapter rounding
  up from this surface against the phase-1 `--line-table` artifact. It
  is listed here only as context for what the surface must support; it
  is not an acceptance criterion of this ticket.
- **Determinism guarantee.** The sim is cycle-counting and owned today.
  Every new control method preserves determinism: no wall-clock, no
  RNG, no global state. The gdbstub's continued execution depends on
  `run`/`run_until` being pure functions of (program, RAM, registers).

Non-goals: no RSP, no breakpoint *table* in sim (the adapter owns it),
no line-granular stepping in sim (adapter, phase 4), no real-hardware
path, and **no PIC14E/PIC18 surface** (those cores stay untouched in
this phase; phase 4 v1 is PIC14 only).

## Acceptance

- `run_until` hits a target address exactly and reports `reached`; a
  program that never lands there reports `halted` at the halt.
- Register get/set round-trips: a written bank-select bit changes the
  bank `bank_base` reports; `PC` write resumes from the written address.
- Memory read/write round-trips across a bank boundary (a direct
  operand at `banked_addr(f) + bank_base` reads the byte written), and
  program-flash read returns the correct word.
- Step advances one instruction; a bounded `run_until` stops after the
  cap with the correct reason.
- Determinism: two `run_until` calls from identical state with the same
  arguments produce byte-identical RAM and the same stop reason.
- Existing sim test suite stays green.

## Notes for the reader

Phase 3 is the bridge phase: it owns the sim-side control surface, but
source awareness (lines via phase 1's `--line-table`, variables via
phase 2's table) is deliberately split out to phase 4's adapter. That
split is what keeps phase 3 small and testable in isolation, and it is
why line-stepping acceptance belongs in phase 4, not here. The real
scheduling constraint is the `crates/sim` area conflict with pic14e P1
(`#246`); sequence phase 3 after `#246` lands or split the file
ownership explicitly.
