# Provenance

Shared snapshot from `apojomovsky/epic-hal` at commit
`51a048421ca28ebf539251957b895946ade680b8` (origin/master on 2026-10-01): every file below is byte-identical to that commit
 (epic-cc#836). The PIC18 demo fixtures that use it
(`hal-pic18-menu-demo`, `hal-pic18-control-demo`, `hal-pic18-pid`,
`hal-pic18-bridge-demo` on `18F4550`, epic-cc toolchain file sets)
compile the size ladder from here.

This is a **snapshot**, not a live sync: epic-cc's own CI must not
depend on epic-hal's current state, so nothing here updates
automatically when epic-hal changes. Re-vendor deliberately
(resolve `sources_for` with the epic-cc toolchain, as in
epic-cc#677) when the drift between this snapshot and real
epic-hal code becomes a concern, e.g. epic-hal adds a new codegen
shape this snapshot does not exercise.
`scripts/check-hal-fixture-drift.py --hal-root <epic-hal checkout>`
fails when any file below differs from the pinned commit, and
reports when the pin itself lags that checkout; see its header.

Scope: the `pic18fxx5x-hal` peripheral/core/epiccc sources and
headers, the `epic-common` headers, and whichever shared-module
files (`epic-taskmgr`, `epic-tick`, `epic-math` plus its `tests`
headers, `epic-pid` library, `epic-serial`, `epic-modbus`,
`epic-bus`, `epic-mcp23x17`, `epic-adcfilter`, shared peripheral
drivers including `ssp`) are identical wherever they are used. One
header lives beside its source and is kept for the same reason:
`pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_tiers_inc.h`.

Files that differ per demo (the `pic18_harness_mdb.c` harness TU),
demo-only modules (`epic-menu-demo`, `epic-control-demo`,
`epic-bridge-demo`, `epic-lcd`, the pid sim TU), and each demo's
generated `config_18F4550.c` live in that demo's own directory;
see its `PROVENANCE.md` for the include order and defines used to
compile it.

Vendored layout keeps the pre-reorg epic-cc names (`epic-*`,
`pic18fxx5x-hal`); upstream renamed them in epic-hal#316
(`lib/*`, `common/*`, `hal/pic18/18fxx5x/*`, `demos/*`). The drift
script maps each file to its upstream path.

Deliberate deltas from the pin (each with an upstream port issue, else
the weekly drift gate stays red until re-vendor absorbs them):

- `epic-bus/src/epic_bus.c`: the I2C/SPI default tables are `static
  const` here (epic-cc#900, port: epic-hal#368); upstream still
  populates mutable tables at init. Re-vendor drops this note.
