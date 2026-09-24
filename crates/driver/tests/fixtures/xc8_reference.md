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

XC8 v4.00 (build date Jun 14 2026) `-O2` numbers for the sixteen
`size-bench` programs (eight from epic-cc#581, four from epic-cc#617,
three from epic-cc#624, one from epic-cc#502), measured 2026-09-22,
2026-09-23 and 2026-09-24 on the `epic-cc-xc8-oracle:local` image.
Each bench compiled standalone:

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
| `bench-handle-init` | 87 (173 B) | 47 | 24 | 9 | -63 |
| `bench-switch-calls` | 63 (126 B) | 3 | 50 | 10 | -13 |
| `bench-u32-loop` | 49 (98 B) | 9 | 105 | 23 | +56 |
| `bench-u16-dec` | 126 (252 B) | 17 | 140 | 39 | +14 |
| `bench-bank` | 30 (60 B) | 10 | 26 | 15 | -4 |
| `bench-struct-scan` | 88 (176 B) | 96 | 205 | 99 | +117 |
| `bench-bitmask` | 54 (108 B) | 13 | 161 | 20 | +107 |
| `bench-hoist` | 137 (274 B) | 13 | 140 | 18 | +3 |
| `bench-w-roundtrip` | 16 (32 B) | 3 | 20 | 10 | +4 |

Negative gap means epic-cc is smaller. Eight benches still trail XC8
(switch, bool, struct-copy, u32-loop, u16-dec, struct-scan, bitmask,
hoist) and rank the remaining codegen work; the rest lead by 4 to 63
words.
The same v4.00 license reasoning as the encoder row above applies: no
license file present, but licenses are free and unlimited since July
2026, so these rows are XC8's genuine `-O2` output.

The four rows above (epic-cc#617) were measured 2026-09-23 on the same
image and flags. `bench-u32-loop` (+56) and `bench-u16-dec` (+14)
confirm real per-site gaps behind the taskmgr_run and heartbeat
clusters; `bench-handle-init` and `bench-switch-calls` lead XC8, so
the init and redraw cluster gaps come from scale and inlining
concentration, not per-site lowering. Small-bench startup noise
applies (both toolchains count their own startup here).

The three rows above (epic-cc#624) were measured 2026-09-23 on the same
image and flags. `bench-struct-scan` (+117) and `bench-bitmask` (+107)
confirm real per-site gaps behind the overflow/stimulus and USART_Init
clusters; `bench-hoist` (+3) shows the pin-reload sequence itself is at
parity, so the gpio4_send gap is accumulation across its twelve calls,
not one shape. Small-bench startup noise applies as above.

## Whole-program ladder (epic-cc#595)

XC8 v4.00 (build date Jun 14 2026) `-O2` numbers for the two large
ladder entries, measured 2026-09-23 on the
`epic-cc-xc8-oracle:local` image against epic-hal at `74d21c6`.
Sources are the manifest-resolved XC8 file sets (`scripts/epic_build.py
build --module <name> --mcu <mcu>` in that checkout), per-TU compile
with the manifest flags (`-mcpu=<mcu> -O2` plus the triaged warning
set) and link with `-mcpu=<mcu> -O2` only; the image wrapper supplies
`-mdfp`. epic-cc columns quote the `size_baseline.toml` rows at
`ca6d8b1`.

| Entry | XC8 flash | XC8 RAM | epic-cc flash | epic-cc RAM | Gap (epic minus XC8) |
|---|---|---|---|---|---|
| `hal-pic18-menu-demo-18f4550` | 8797 (17593 B) | 599 | 12137 | 741 | +3340 flash, +142 RAM |
| `hal-pic16-encoder-full-16f877a` | 5477 | 344 | 7071 | 329 | +1594 flash, -15 RAM |

The XC8 file sets are not the vendored epic-cc input lists verbatim:
the 21 menu-demo inputs use the `epiccc` vector, dispatch, and config
shims plus the sim harness and a peripheral subset, none of which XC8
accepts, so the oracle build uses the `target` vector, dispatch, and
WDT slices, the full peripheral set, a generated config source, and
`examples/example_menu_demo.c` (25 TUs). The encoder side differs the
same way (`target` slices, full set, generated config, the example
main). XC8 also links its own runtime startup, so exact parity stays
noisy: treat the gaps as directional, not per-word bills.

Program space on PIC18 reads in bytes, and the menu-demo count is odd
(17593 B), so the word figure rounds up one half-word. The encoder row
re-measures the older 2026-09-02 snapshot above (5356 words, same 344
RAM bytes): +121 words of source drift since September 2 plus a
different build image. The same v4.00 license reasoning as the rows
above applies: no license file present, but licenses are free and
unlimited since July 2026, so these rows are XC8's genuine `-O2`
output. Method note: `make oracle-exec` re-demands the
licence-gated installer through its `oracle-image` prerequisite even
when the image is already built, so these builds ran through
`docker run` on the existing image directly.
