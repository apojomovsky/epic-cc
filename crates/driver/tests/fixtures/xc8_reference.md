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
vendors from). No license file was present when this was measured, XC8
may be running a fallback optimization tier rather than the requested
`-O2`; see apojomovsky/epic-hal#121.

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
