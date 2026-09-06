# Debugger phase 3: sim control surface

**Ticket body (to become `epic-cc#NNN`)** — mirror `#246`'s structure.

---

## Summary

Third phase of the source-level debugger (`docs/34-debugger-design.md`
§4; umbrella `#203`). Phase 1 gave the address-to-line table; phase 2
gives the typed variable table. This phase gives `crates/sim` a
control surface the phase-4 gdbstub drives: run-until-address,
register read/write, memory read/write, and instruction-granularity
step with breakpoint stops. Scope for v1 (settled 2026-09-06):
**PIC14 core only** (`Pic14` on the F877A), proving the pattern per
`docs/34` §5 non-goal "PIC18. Land the PIC14 path first."

Depends on phase 2's type table (for `print <name>` to know where a
variable lives, `info address` needs the join). Blocked by: none.
Umbrella: `#203`.

## Scope

- **`run_until` primitive.** Run instructions until `pc` reaches a
  target address, the program halts, or a step cap is hit. Returns the
  stop reason (reached / halted / capped). This is the breakpoint
  mechanism: the gdbstub sets a watch on an address, the sim runs
  until it lands there. No in-sim breakpoint table needed; the adapter
  (phase 4) owns the set.
- **Register surface.** Expose reads of `W`, `PC`, `STATUS` (with the
  active bank derived from STATUS `RP1:RP0` per `bank_base()`), `FSR`
  (`0x04`), `PCLATH`, `INTCON`, and writes to `W`, `PC`, the bank
  select bits. The sim already keeps `pc`, `w`, `ram` fields and a
  `bank_base()`; this phase adds a coherent public accessor set rather
  than raw field pokes.
- **Memory read/write.** A uniform `read_mem(addr, len)` /
  `write_mem(addr, bytes)` over the RAM image (`ram`/`ram_mut`),
  banked-resolved the same way `banked_addr` resolves direct operands,
  plus read access to program flash (`prog`) since DWARF const arrays
  and the PC's instruction stream live there.
- **Step with stop semantics.** `step` already exists; it advances one
  instruction. Phase 3 adds a stepping primitive the adapter can call
  one-at-a-time and a way to run until the *next* distinct source line
  (line stepping rounds up in the adapter, per `docs/34` §4 phase 3;
  `sim` itself stays instruction-granular).
- **Determinism guarantee.** The sim is cycle-counting and owned today.
  Every new control method must preserve determinism: no wall-clock,
  no RNG, no global state. The gdbstub's continued execution depends
  on `run`/`run_until` being pure functions of (program, RAM, registers).

Non-goals: no RSP, no breakpoint *table* in sim (the adapter owns it),
no real-hardware path, and **no PIC14E/PIC18 surface** (those cores
stay untouched in this phase; phase 4 v1 is PIC14 only).

## Acceptance

- `run_until` hits a target address exactly and reports `reached`;
  a program that never lands there reports `halted` at the halt.
- Register get/set round-trips: a written bank-select bit changes the
  bank `bank_base` reports; `PC` write resumes from the written address.
- Memory read/write round-trips across a bank boundary (a direct
  operand at `banked_addr(f) + bank_base` reads the byte that was
  written), and program-flash read returns the correct word.
- Step advances one instruction; running to "next source line" (the
  adapter's rounding) stops on the first instruction whose address
  is on a different line per the phase-1 line table, and stops before
  crossing a line on a `BANKSEL` that inherits a preceding line (per
  the phase-1 preservation contract).
- Determinism: two `run_until` calls from identical state with the same
  arguments produce byte-identical RAM and the same stop reason.
- Existing sim test suite stays green.

## Notes for the reader

Phase 1's line table (the `--line-table` artifact) is what makes
line-granular stepping possible: the adapter compares each candidate
address against the table to decide when a step crosses a line. The
"run to next source line" behavior therefore lands here (sim drives it)
but the *decision* is a phase-1-table lookup, so this phase must read
the line table. Phase 3 has no source-variable knowledge; that is
phase 2's table, consumed by phase 4.
