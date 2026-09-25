# Provenance

Shared snapshot from `apojomovsky/epic-hal` on 2026-09-25 (commit
`879834c`): every source and header below is byte-identical across
the PIC18 demo fixtures that use it (`hal-pic18-menu-demo`,
`hal-pic18-control-demo`, `hal-pic18-pid` on `18F4550`, epic-cc
toolchain file sets), for the size ladder (epic-cc#677).

This is a **snapshot**, not a live sync: epic-cc's own CI must not
depend on epic-hal's current state, so nothing here updates
automatically when epic-hal changes. Re-vendor deliberately
(resolve `sources_for` with the epic-cc toolchain, as in
epic-cc#677) when the drift between this snapshot and real
epic-hal code becomes a concern, e.g. epic-hal adds a new codegen
shape this snapshot does not exercise.

Scope: the `pic18fxx5x-hal` peripheral/core/epiccc sources and
headers, the `epic-common` headers, and whichever shared-module
files (`epic-taskmgr`, `epic-tick`, `epic-math`, `epic-pid` library,
`epic-serial`, shared peripheral drivers) are identical wherever
they are used. One header lives beside its source and is kept for
the same reason:
`pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_tiers_inc.h`.

Files that differ per demo (the `pic18_harness_mdb.c` harness TU),
demo-only modules (`epic-menu-demo`, `epic-control-demo`,
`epic-adcfilter`, `epic-lcd`, the pid sim TU, the `ssp` driver), and
each demo's generated `config_18F4550.c` live in that demo's own
directory; see its `PROVENANCE.md` for the include order and
defines used to compile it.
