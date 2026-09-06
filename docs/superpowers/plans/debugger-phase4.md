# Debugger phase 4: gdbstub adapter + ELF/DWARF sidecar

**Ticket body (to become `epic-cc#NNN`)**. Mirror `#246`'s structure.
Whether the two halves (sidecar encoder, RSP adapter) are developed on
one worktree or two is a filing-time decision per the repo workflow
(one PR per issue, `Closes #N`); this draft does not prescribe it.

---

## Summary

Fourth and final phase of the source-level debugger
(`docs/34-debugger-design.md` §4; umbrella `#203`). The gdbstub/RSP
adapter (front-end decided in the 2026-09-05 brainstorm) over phase
3's sim control surface, emitting the ELF+DWARF sidecar from phases
1-2's data, shipped as a **separate `epic-cc-gdbserver` binary**.
Scope for v1 (settled 2026-09-06): **PIC14 core only**, proving the
pattern before the PIC18 path (`docs/34` §5 non-goal).

Depends on phase 1 (line table, landed), phase 2 (type table), phase 3
(sim control surface). Umbrella: `#203`.

## Scope

- **Open-risk spike first.** `docs/34` §3 records as not-yet-spiked:
  "PIC14 is not a gdb-known architecture." The design must confirm a
  custom-architecture target works: gdb's `qXfer:features:read`
  target-description XML (the established pattern) combined with a
  synthetic `e_machine` in the sidecar ELF. Follow the §3 spike
  methodology (a throwaway stub answering raw RSP with fabricated
  values, on a known gdb arch first, then the custom-arch
  interaction). The phase is not implemented until this lands.
- **`epic-cc-gdbserver` binary.** A new crate/bin, decoupled from the
  one-shot compiler lifecycle (the compiler emits HEX once and exits;
  a debugger is a long-lived network service). Entry:
  `epic-cc-gdbserver <hex> <sidecar.elf> --port <port>`, then
  `gdb <sidecar.elf> -ex "target remote :<port>"`.
- **gdbstub::Target** over phase 3's control surface. Implement the
  registers (PIC14 `W`/`PC`/`STATUS`/`FSR`/active bank / the six
  bank-independent core registers), memory read/write, and run/step
  (with the phase-3 `run_until` as breakpoint execution). This phase
  also implements **line-granular stepping**: the adapter rounds up
  from the phase-3 surface against the phase-1 `--line-table` artifact,
  i.e. stop on the first instruction whose address is on a different
  line, respecting the phase-1 `BANKSEL` inherits-a-line case.
- **Sidecar encoder with aggregate DWARF.** The encoder (proposed
  `gimli::write` for DWARF, `object` crate for the ELF container)
  consumes the phase-2 **in-process type table** (the single source of
  truth; the phase-2 text artifact is not the input) plus the phase-1
  line table. Every location is `DW_OP_addr(constant)` (`docs/34` §1),
  so there is no CFI, frame-base, or register-relative DWARF. The v1
  types are scalars **plus aggregates**, so the emitted DIE set is not
  flat base types only: it must include `DW_TAG_structure_type` /
  `DW_TAG_union_type` with `DW_TAG_member` +
  `DW_AT_data_member_location`, `DW_TAG_array_type` +
  `DW_TAG_subrange_type`, and `DW_TAG_enumeration_type`, for
  `print s.member` / `print arr[i]` to resolve. Minimal in the
  dimension that matters (one compile unit, constant-address
  locations), not minimal in DIE-kind coverage.
- **Add gdb to the dev/ci image.** Phase 4's acceptance runs real gdb
  sessions, and the dev image (`Dockerfile` `dev` stage) does not
  install gdb; it is absent from the toolchain contract everywhere in
  the repo. This phase adds a digest-pinned apt gdb to the image so
  the acceptance is runnable in CI. gdb is a test/acceptance-time
  dependency for the build and a user-side dependency for whoever runs
  the debugger; it is **decided at filing/implementation time** whether
  gdb ships in the release bundle or stays a build/CI-time host dep
  (a host that runs `epic-cc-gdbserver` needs a gdb either way to make
  it useful, but the bundle itself does not have to carry one).

Non-goals (v1): real-hardware debugging (ICSP / Microchip debug
executive, `docs/34` §5), any architecture beyond PIC14, and any
location form beyond constant addresses.

## Acceptance

- The spike confirms a custom-arch gdb session against the sim: at
  minimum `gdb <sidecar.elf> -ex "target remote :<port>" -ex "info
  registers"` lists PIC14 registers, and `x/4xb 0x70` reads ram. The
  spike's findings are recorded and the design doc is updated before
  the adapter is built.
- gdb is present in the dev/ci image (CI runs the acceptance).
- `epic-cc-gdbserver <hex> <sidecar.elf> --port <p>` accepts a
  connection and an end-to-end gdb session: set a `break t.c:N`
  (resolved from the sidecar `.debug_line`), `run`, stop at the line,
  `print <var>` (typed, from the sidecar DWARF), `print s.member` /
  `print arr[i]` for an aggregate fixture, `continue`/`step` to the
  next line, `quit`. The sidecar's `.debug_line` matches phase 1's
  `--line-table` artifact for the same fixture.
- Step crosses a source line correctly for the phase-1 `BANKSEL`
  inherits-a-line case (line-stepping respects the preservation
  contract).
- The sidecar for a fixture that exercises a global, a local, and an
  array/struct element reads the right memory address over RSP (the
  phase-1/§3 pattern: a fabricated address + DWARF type resolves to
  correct values).
- Regression: sim tests and driver tests stay green; `--line-table`
  output unchanged for existing fixtures.
- Release bundles include the `epic-cc-gdbserver` binary.

## Notes for the reader

Phase 4 is the only phase whose design doc carries an unresolved risk,
so its ticket deliberately front-loads the spike: the whole point of
`docs/34` §3's spike methodology is to retire that risk *before* the
adapter is built, not to discover a custom-arch dead end after phases
2-3 are invested. If the spike shows the custom-arch path is a dead
end, the fallback is a PIC-known-arch `e_machine` hack or a
target-description-only session with raw address debugging; the spike
decides which is real. The gdb-in-image change and the aggregate-DIE
encoder are the two under-scoped pieces this ticket now makes explicit.
