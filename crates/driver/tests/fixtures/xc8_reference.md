# XC8 size reference (epic-cc#200)

Last-known XC8 numbers for the `size_regression_e2e.rs` ladder entries
that have a real XC8-buildable counterpart, for comparison against
epic-cc's own tracked baseline (`size_baseline.toml`). **Not run as part
of this suite or any epic-cc CI**: XC8 is proprietary and deliberately
absent from epic-cc's own toolchain image (ADR-006, XC8 is a
differential oracle only, never a build dependency). These numbers are a
snapshot, refresh them deliberately when they matter (an XC8 upgrade, a
fixture change), not on every run.

The `add-*`, `hal-pic16-blink-*`, and `hal-pic18-blink-*` ladder entries
are epic-cc-only micro-fixtures with no matching real-module XC8 build,
so they have no row here.

| Entry | XC8 flash | XC8 RAM | XC8 version | Measured |
|---|---|---|---|---|
| `hal-pic16-encoder-full-16f877a` | 5356/8192 words (65.4%) | 344/368 bytes | v4.00 build 20260614213421 | 2026-09-02 |

Source: epic-hal's `epic-encoder` module, `make xc8-build MODULE=epic-encoder
MCU=16F877A` (same combination `hal-pic16-encoder-full/PROVENANCE.md`
vendors from). The exact `xc8-cc` invocation `epic_build.py` emits for
this module/device:

```
xc8-cc -mdfp=<PIC16Fxxx_DFP> -mcpu=16f877a -O2 -std=c99 -Wall -Wextra \
  -Wno-520 -Wno-2053 -Wno-759 -Wno-1516 -Wno-1311 -Wno-1262 -Wno-1510 \
  -Wno-2098 -Wno-1498 -Wno-unused-function -Wno-unused-variable \
  -Wno-unused-parameter -Wno-sign-conversion -Wno-implicit-int-conversion \
  -DPIC16F877A -I<includes> -DFOSC_HZ=20000000
```

This row was originally flagged as possibly reflecting a degraded,
unlicensed-fallback optimization tier rather than the requested `-O2`
(no license file was present, and `xc8-cc --help` documents a
`--nofallback` flag whose description implies exactly such a fallback
exists). That concern is resolved: XC8 v4.00's own release notes state
license checks were removed entirely (build date matching what's
measured here), and as of July 2026 all MPLAB XC compiler licenses are
free and unlimited, so `--nofallback` being `deprecated` is a leftover
from the pre-v4 licensing model, not evidence of an active one, and no
license file being present is no longer unusual. The `Og9` label the
`.s` output carries (`subtitle "Microchip MPLAB XC8 C Compiler v4.00
build ... Og9"`) is very likely an internal build/pipeline identifier
unrelated to optimization level. This row is very likely already XC8's
genuine best output for the requested `-O2`. See apojomovsky/epic-hal#121
(closed) for the full correction.

## Regenerating

Tooling for a one-command regeneration lives in epic-hal
(apojomovsky/epic-hal#122), since that's where the XC8 toolchain and
build scripts already live. Until that lands, the manual steps used to
produce the row above:

```
make image                                    # epic-hal-toolchain:local, needs
                                               # vendor/xc8-installer.run +
                                               # vendor/mplabx-installer.tar
make xc8-build MODULE=epic-encoder MCU=16F877A
```

then read the flash/RAM line from the `xc8-cc` link step's own
`16F877A Memory Summary` output.

## Microbench ladder (epic-cc#581)

XC8 v4.00 (build date Jun 14 2026) `-O2` numbers for the eight
`size-bench` programs, measured 2026-09-22 on the
`epic-cc-xc8-oracle:local` image. Each bench compiled standalone:

```
xc8-cc -mcpu=18f4550 -O2 bench-<stem>.c -o <stem>.hex
```

Program space bytes halve to flash words (PIC18 words are 2 bytes);
Data space bytes are RAM bytes directly. epic-cc columns quote the
`size_baseline.toml` rows of the same date; the switch row still
predates #590 (94 words, 64 after it merges).

| Bench | XC8 flash | XC8 RAM | epic-cc flash | epic-cc RAM | Gap (epic minus XC8) |
|---|---|---|---|---|---|
| `bench-shift` | 108 (216 B) | 16 | 85 | 26 | -23 |
| `bench-wide-const` | 28 (56 B) | 6 | 16 | 10 | -12 |
| `bench-zero-init` | 38 (76 B) | 12 | 18 | 16 | -20 |
| `bench-struct-copy` | 36 (72 B) | 41 | 40 | 51 | +4 |
| `bench-switch` | 55 (110 B) | 2 | 94 (64 post-#590) | 8 | +39 (+9 post-#590) |
| `bench-bool` | 29 (58 B) | 2 | 36 | 9 | +7 |
| `bench-dead-store` | 16 (32 B) | 3 | 12 | 8 | -4 |
| `bench-bank` | 30 (60 B) | 10 | 26 | 15 | -4 |

Negative gap means epic-cc is smaller. Three benches still trail XC8
(switch, bool, struct-copy) and rank the remaining codegen work; the
rest lead by 4 to 23 words. The same v4.00 license reasoning as the
encoder row above applies: no license file present, but licenses are
free and unlimited since July 2026, so these rows are XC8's genuine
`-O2` output.
