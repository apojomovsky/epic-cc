# SDCC regression-suite pass rate as nightly context (Tier 3)

Ticket: epic-cc#318. Design of record: docs/35 section 4, corpus Tier 3.

## Goal

A nightly artifact reports SDCC's own pic14/pic16 regression-suite pass
rate next to the parity table, with the suite provenance (SDCC version)
recorded. This is context for the parity comparison, not a gate: SDCC's
own pic ports have acknowledged bugs (docs/35 section 1), so the pass
rate is informational, never fail-on-regression.

## What already exists

- The SDCC 4.6.0 regression suite is vendored into the image at
  `$PIC8_SDCC_REGRESSION` (`/usr/local/share/sdcc/regression`), GPL,
  never committed (Dockerfile).
- The nightly workflow (`.github/workflows/nightly.yml`) already runs
  the P0 parity differential (`scripts/sdcc-parity.sh`) and writes its
  table to the step summary.
- gpsim is NOT in the image; the vendored suite's pic14/pic16 ports use
  gpsim as their emulator.

## The blocker: `#pragma preproc_asm -` + `__asm` under SDCC 4.6.0

Both `ports/pic14/support.c` and `ports/pic16/support.c` start with
`#pragma preproc_asm -` and contain `__asm`/`__endasm` blocks. Under
SDCC 4.6.0 this combination fails to compile: every line after the asm
block errors with `error 329: stray character`. Reproduced minimally
(`#pragma preproc_asm -` + any `__asm` block + any following C line).

Root cause: with `preproc_asm` off, `_sdcpp_skip_asm_block` in
`support/cpp/libcpp/lex.cc` consumes the `__endasm` terminator but the
following C line is then mis-tokenized as stray characters. The `+`
form works but mangles the `;;` gpsim comment lines inside the asm
blocks (they are not valid C comments under `+`).

Workaround (image-only, never committed): patch the two `support.c`
files in the image at build time to use `#pragma preproc_asm +` and
strip the `;;` comment lines. Verified: the gpsim `.direct` directives
survive, and the suite compiles and runs.

## Design

1. **Dockerfile**: add `gpsim` to the test-time apt layer (it is a
   test oracle, GPL, image-only, consistent with the existing gpsim
   note in docs/35 section 4). Patch the two vendored `support.c`
   files in the SDCC RUN (image-only, never committed) with the
   workaround above.

2. **New script `scripts/sdcc-regression.sh`**: runs the vendored suite
   for pic14 and pic16 under SDCC, parses each port's `.sum` summary
   (failures / tests / test cases), and writes a pass-rate line to the
   step summary next to the parity table, stamped with the SDCC version
   (provenance). The suite is run with `make test-pic14` /
   `make test-pic16` in the vendored dir, `SDCC_BIN_PATH=/usr/local/bin`.

3. **Nightly workflow**: add a step to the existing `sdcc-parity` job
   (or a sibling job) that runs `scripts/sdcc-regression.sh` after the
   differential, so the pass rate lands in the same nightly artifact.

## DoD

- Nightly artifact reports the SDCC pic14/pic16 regression pass rate
  next to the parity table, with the SDCC version recorded.
- The suite runs in the image (gpsim present, support.c patched).
- No SDCC/GPL code committed; the patch is image-only.

## Non-goals

- Adapting the suite to run under epic-cc (the stretch goal) is a
  separate ticket.
- The pass rate is informational, not a CI gate.
