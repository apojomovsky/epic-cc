# speed-bench provenance

Cycle-count microbenchmarks for the epic-cc#840 ladder
(`crates/driver/tests/cycle_ladder_e2e.rs`, baseline
`fixtures/cycle_baseline.toml`). Hand-written, one file per sink, each
timed through the marker protocol below.

## Marker protocol

Every kernel defines `volatile unsigned char bench_mark` and its `main`
writes `1` before the work and `2` after. The harness resolves the
address from the compiler's own `--map` output
(`global bench_mark 0xNN`), steps the simulator to each store, and
records the cycle delta. No function addresses are needed, so the same
procedure works in MPLAB SIM: load the identical HEX, watch the data
address from the MAP file, and read the stopwatch at each write.

The markers are plain volatile stores, so they compile unchanged under
the reference compiler. Anti-folding rule every kernel follows: inputs
are volatile globals with nonzero initializers, loaded inside the timed
region; results are sunk to a volatile global before the `2` store. The
load, compute, sink chain is data-dependent, so the optimizer cannot
hoist the work above the `1` or sink it below the `2`, under either
profile.

## ISR fixtures

`speed-isr-*.c` use a two-flag variant: main raises `bench_sync` to `1`
and spins on a volatile counter, giving the harness an injection
window that survives codegen shifts (no hardcoded PCs). The ISR's first
statement stores `bench_isr = 1`. Latency is the vector-to-first-store
delta across `fire_interrupt`; the round trip is the halt-cycle delta
of an interrupted run against a clean baseline run.

## What each kernel pins

- `speed-mul-u8/u16/u32`: the widening multiply per width.
- `speed-divmod-u8/u16/u32`: software division both cores lower to.
- `speed-shift`: variable-count shifts with unknown counts.
- `speed-memcpy-32`, `speed-memset-32`: 32-byte block ops plus their loops.
- `speed-strlen-strcmp`: string walk plus full-length compare.
- `speed-crc16`: CRC-16/CCITT bit loop, the bridge-demo per-frame shape.
- `speed-switch16`: 16-case dispatch, the menu event-switch shape.
- `speed-struct-scan`: struct-array walk with a non-power-of-two stride.
- `speed-u16-dec`: divide/modulo-by-10 decimal engine, the heartbeat shape.
- `speed-pid-step`: self-contained Q8.8 PI arithmetic (seeds positive,
  so no shift of a negative value is needed).
- `scen-pid-update`: one `epic_pid_update` through the vendored epic-pid
  sources (PIC18 only, like the demo it comes from).
- `speed-isr-*`: interrupt latency plus round trip per core.

Menu-tick and control-pass scenarios are deliberately absent: both run
through `epic_tick_delay_ms`, which spins on a timer flag the sim does
not model yet (epic-cc#859). They become follow-up rows once the timer
model lands.
