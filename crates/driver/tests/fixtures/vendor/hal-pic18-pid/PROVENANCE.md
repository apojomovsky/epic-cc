# Provenance

Vendored from `apojomovsky/epic-hal` on 2026-09-25 (commit `879834c`),
the `epic-pid` module's sim variant on `18F4550` (epic-cc toolchain
file set), for the size ladder (epic-cc#677).

This is a **snapshot**, not a live sync: epic-cc's own CI must not
depend on epic-hal's current state, so nothing here updates
automatically when epic-hal changes. Re-vendor deliberately
(resolve `sources_for` with the epic-cc toolchain and regenerate
the config via `scripts/epic_build.py`, as in epic-cc#677) when the
drift between this snapshot and real epic-hal code becomes a
concern, e.g. epic-hal adds a new codegen shape this snapshot does
not exercise.

Only this demo's own files live here (the sim TU, the pid-only
`ssp` peripheral driver, this demo's copy of the
`pic18_harness_mdb.c` harness TU, and the generated config).
Everything byte-identical across the PIC18 demo fixtures lives once
in `../hal-pic18-base/`; see its `PROVENANCE.md` for the shared
scope.

Demo files (paths relative to this directory):

```
epic-pid/tests/sim_pid.c
pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c
pic18fxx5x-hal/src/peripherals/pic18fxx5x_ssp.c
```

`config_18F4550.c` is a copy of the `EPIC_CONFIG(...)` translation unit
`scripts/epic_build.py` generates for this module/device in epic-hal
(`--toolchain epic-cc` variant, byte-identical to the menu-demo one);
it does not exist as a checked-in file there.

Include order used to compile this fixture (device `18F4550`): all
under `../hal-pic18-base/`, since this demo adds no include tree of
its own:

```
-I../hal-pic18-base/pic18fxx5x-hal/include/epiccc
-I../hal-pic18-base/pic18fxx5x-hal/include
-I../hal-pic18-base/epic-common/include
-I../hal-pic18-base/epic-math/include
-I../hal-pic18-base/epic-pid/include
-I../hal-pic18-base/epic-tick/include
-I../hal-pic18-base/epic-serial/include
```

Defines: `PIC18F4550`, `FOSC_HZ=48000000`, `__EPIC_CC__`.
