# ADR-019 -- PIC variant support: file-per-device TOML + build.rs codegen + --target

**Status:** Proposed 2026-08-22 (pending user approval of the spec)<br>
**Decides:** `epic-cc#83` + `#84` + `#85` scope<br>
**Supersedes:** `ADR-004` "Device support is data, not code" (TOML sketch) and `docs/29` D-3's "two structs behind one selector" interim; not a reversal, a completion<br>
**Parent:** `docs/31-ecosystem-integration-design.md` D-4, `docs/superpowers/specs/2026-08-22-pic-variants-design.md`

## Decision

* Each supported part is one TOML under `crates/device/devices/<name>.toml` with `name`, `core`, `flash_words`, `ram_banks`, `common_ram`, `stack_depth`, `interrupt_vectors`, `config` (plus optional `sfrs` table for the HAL contract). Details in spec §4.
* `crates/device/build.rs` globs `devices/*.toml`, validates invariants, and emits `OUT_DIR/devices.rs` with `pub const` blocks + `ALL` + `by_name()`. `src/lib.rs` `include!`s it. `Device` stays `&'static`.
* `fsr_window` derives from `Device::ram_banks`/`common_ram`; no PIC14 literals remain in `isel`. The simulator is not fully derived: it still decides banked GPR by a fixed `0x20-0x6F` operand window, tracked in `#95`.
* Driver gains `--target <name>` / `-mcu <name>` resolving via `by_name`; unknown name lists `ALL`. `PlatformIO` `boards/*.json` maps `mcu` to that string (PIO-1).
* CI is stratified: canonical per core (full), per-device lightweight only for TOMLs touched in the PR, nightly lightweight for all (spec §8). Full `devices × fixtures` never.

## Rationale

* A PlatformIO user knows a datasheet, not Rust. One TOML per part + `git diff --name-only` gives "changed devices" for CI without editing a central Rust file.
* Codegen, not runtime parse, keeps `&Device` `&'static` and every consumer signature unchanged, while schema + `build.rs` panics give precise "bad TOML" messages.
* A bug is core-wide or data-wide. Canonical-per-core finds core bugs; schema + one sanity compile finds data bugs. Linear CI is the HAL trap and is avoided here.

## Alternatives rejected

* **Stay with hand-written Rust consts** -- zero tooling but every addition edits a Rust file and needs a registry hand-edit. Retained only until `build.rs` lands.
* **Runtime-parsed TOML** -- would force `Device` to own `String`/`Vec`, threading owned values through `alloc`/`isel`/`sim` for no benefit.

## Consequences

* `p16f887.toml` is the first exemplar; `pic14e` (`p16f1937` etc.) stayed a firewall error until the `isel-pic14e` backend landed. The firewall was at the driver, not at codegen: `build.rs` validates a `pic14e` TOML like any other (same single `0x0004` vector as `pic14`) and the driver refused the target with `core pic14e which has no backend yet`. Refusing at codegen would make a `pic14e` TOML impossible to keep in tree at all, which was the wrong trade: the data had to be reviewable before the backend existed. Each backend stage carries the same firewall as a backstop (`fosc`, `fuzz`); the driver-level firewall and the `asm` backstop lifted when the P2 integer spine (`isel-pic14e` + BSR/`MOVLB` banking) landed.
* DFP/ATDF -> TOML generator is a follow-up (#86); `.atdf` itself is never committed, only the TOML it generates (licence-clean, same posture as config transcription today).

## Revisit if

* `hal` needs SFR names that cannot be expressed in the optional `sfrs` table, or a new core requires fields not in the schema -- extend the schema, don't revert to Rust consts.

## Addendum 2026-08-23 -- DFP as the TOML source (epic-cc#86)

`scripts/gen-device.py` is the one-shot generator that removes the
hand-transcription tax at device #3+. It reads a Microchip DFP
(`*.atdf` / `*.PIC` / `xc8/pic/dat/{ini,cfgdata}`) and emits the
deterministic `crates/device/devices/<stem>.toml`.

* **Primary source:** the ATDF/EDC PIC file from the DFP
  (`PIC16Fxxx_DFP` on https://packs.download.microchip.com/), free
  download, gitignored; `gputils` `.inc` is the byte-for-byte oracle.
* **Fallback:** `xc8/pic/dat/ini/*.ini` + `cfgdata/*.cfgdata` under
  `$PIC8_XC8_ROOT` (the same DFP repacked by XC8), which is what
  `gen-device` uses when `--atdf` is not given and a local XC8
  install is present.
* **Posture:** XC8 headers are black-box oracle only, per `AGENTS.md`
  GPL boundary. The `.atdf`/`.PIC` itself is **never committed**,
  only the TOML it generates, same licence posture as the original
  hand transcription.
* **Alias table:** DFP field/value names are normalised to our
  `EPIC_CONFIG` names (`WDTE` -> `wdt`, `FOSC` -> `osc`, `INTRC` ->
  `intosc`, etc.) documented in the script header; `gen-device
  --check` + `git diff --exit-code` in CI/nightly gates drift.

## Addendum 2026-09-09 -- Peripheral blindness is a decision, not an accident (epic-cc#333)

The schema above is closed over what the compiler needs to place code
and data and resolve config fuses: `core`, `flash_words`, `ram_banks`,
`common_ram`, `stack_depth`, `interrupt_vectors`, `config`, the
PIC18-only `access_bank`/`fixed_retval` layout facts, and the optional
`sfrs` table, which exists only for the HAL contract and is never
consulted by `isel`, `asm` or `sim`. Peripheral register semantics
(UART/SPI/ADC/timer register layouts, peripheral counts, pin muxing)
are permanently out of scope for `Device`; `epic-hal` owns them. This
is not a new restriction; it is what the decision above already built
and what has kept every device addition single-file: the largest
family is no harder to keep in the registry than the smallest because
the compiler's device model is deliberately blind to almost everything
that varies member-to-member. If `isel` ever needs a peripheral fact,
that is the signal to revisit this decision, not a normal device
addition. Written down per `docs/38` D-4; the four-core closed set
(Baseline, Mid-Range, Enhanced Mid-Range, PIC18) is stated in
`docs/08`'s device-registry bullet with the Microchip citation.
