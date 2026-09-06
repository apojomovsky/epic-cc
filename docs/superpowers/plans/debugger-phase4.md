# Debugger phase 4: gdbstub adapter + ELF/DWARF sidecar

**Ticket body (to become `epic-cc#NNN`)** — mirror `#246`'s structure.
Splits into two issues once the open-risk spike (below) resolves: one
for the target-description spike + RSP adapter, one for the sidecar
encoder, on the same `feat/<issue>-gdbstub` worktree branch.

---

## Summary

Fourth and final phase of the source-level debugger
(`docs/34-debugger-design.md` §4; umbrella `#203`). The gdbstub/RSP
adapter (front-end decided in the 2026-09-05 brainstorm) over phase
3's sim control surface, emitting the ELF+DWARF sidecar from phases
1-2's tables, shipped as a **separate `epic-cc-gdbserver` binary**.
Scope for v1 (settled 2026-09-06): **PIC14 core only**, proving the
pattern before the PIC18 path (`docs/34` §5 non-goal).

Depends on phase 1 (line table, landed), phase 2 (type table), phase 3
(sim control surface). Umbrella: `#203`.

## Scope

- **Open-risk spike first.** `docs/34` §3 records as not-yet-spiked:
  "PIC14 is not a gdb-known architecture." The design must confirm a
  custom-architecture target works: gdb's `qXfer:features:read`
  target-description XML (the established pattern for architectures
  that predate built-in gdb support) combined with a synthetic
  `e_machine` in the sidecar ELF. This spike is a precondition for
  finalizing this phase's design; phase 4 is not implemented until it
  lands. Follow the §3 spike methodology (a throwaway stub answering
  raw RSP with fabricated values, on a known gdb arch first, then the
  custom-arch interaction).
- **`epic-cc-gdbserver` binary.** A new crate/bin, decoupled from the
  one-shot compiler lifecycle (the compiler emits HEX once and exits;
  a debugger is a long-lived network service). Entry:
  `epic-cc-gdbserver <hex> <sidecar.elf> --port <port>`, then
  `gdb <sidecar.elf> -ex "target remote :<port>"`. Ships alongside
  `epic-cc` in the release bundles.
- **gdbstub::Target** over phase 3's control surface. Implement the
  registers (PIC14 `W`/`PC`/`STATUS`/`FSR`/active bank / the six
  bank-independent core registers), memory read/write, and run/step
  (with the phase-3 `run_until` as breakpoint execution).
- **Target description + sidecar encoder.** The DWARF/ELF encoder
  (proposed `gimli::write` for DWARF, `object` crate for the ELF
  container) consumes the phase-1 line table and the phase-2 typed
  variable table. Given every location is `DW_OP_addr(constant)`
  (`docs/34` §1), the emitted DWARF is near-minimal: one compile unit,
  flat `DW_TAG_subprogram`/`DW_TAG_variable`/`DW_TAG_base_type`,
  a `.debug_line` program from the line table. The sidecar is never
  flashed and does not touch the HEX output path.

Non-goals (v1): real-hardware debugging (ICSP / Microchip debug
executive, `docs/34` §5), any architecture beyond PIC14, and any
location form beyond constant addresses.

## Acceptance

- The spike confirms a custom-arch gdb session against the sim: at
  minimum `gdb <sidecar.elf> -ex "target remote :<port>" -ex "info
  registers"` lists PIC14 registers, and `x/4xb 0x70` reads ram. The
  spike's findings are recorded before the adapter is built, and the
  design doc is updated with the result.
- `epic-cc-gdbserver <hex> <sidecar.elf> --port <p>` accepts a
  connection and an end-to-end gdb session: set a `break t.c:N`
  (resolved from the sidecar `.debug_line`), `run`, stop at the line,
  `print <var>` (typed, from the sidecar DWARF), `continue`/`step` to
  the next line, `quit`. The sidecar's `.debug_line` matches phase 1's
  `--line-table` artifact for the same fixture.
- The sidecar for a fixture that exercises a global, a local, and a
  `print`ed array element reads the right memory address over RSP
  (the phase-1/§3 pattern: a fabricated address + DWARF type must
  resolve to correct values).
- Regression: sim tests and driver tests stay green; `--line-table`
  output unchanged for existing fixtures.
- Release bundles include the `epic-cc-gdbserver` binary.

## Notes for the reader

Phase 4 is the only phase whose design doc carries an unresolved risk,
so its ticket deliberately front-loads the spike: the whole point of
`docs/34` §3's spike methodology is to retire that risk *before* the
adapter is built, not to discover a custom-arch dead end after phases
2-3 are invested. If the spike shows the custom-arch path is a dead
end, the fallback is a PIC-known-arch `e_machine` hack or a gdb
target-description-only session with raw address debugging; the spike
decides which is real.
