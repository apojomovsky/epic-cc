# ADR-019 -- PIC variant support: file-per-device TOML + build.rs codegen + --target

**Status:** Proposed 2026-08-22 (pending user approval of the spec)<br>
**Decides:** `epic-cc#83` + `#84` + `#85` scope<br>
**Supersedes:** `ADR-004` "Device support is data, not code" (TOML sketch) and `docs/29` D-3's "two structs behind one selector" interim; not a reversal, a completion<br>
**Parent:** `docs/31-ecosystem-integration-design.md` D-4, `docs/superpowers/specs/2026-08-22-pic-variants-design.md`

## Decision

* Each supported part is one TOML under `crates/device/devices/<name>.toml` with `name`, `core`, `flash_words`, `ram_banks`, `common_ram`, `stack_depth`, `interrupt_vectors`, `config` (plus optional `sfrs` table for the HAL contract). Details in spec §4.
* `crates/device/build.rs` globs `devices/*.toml`, validates invariants, and emits `OUT_DIR/devices.rs` with `pub const` blocks + `ALL` + `by_name()`. `src/lib.rs` `include!`s it. `Device` stays `&'static`.
* `fsr_window` and the simulator map derive from `Device::ram_banks`/`common_ram`; no PIC14 literals remain in `isel`.
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

* `p16f887.toml` is the first exemplar; `pic14e` (`p16f1937` etc.) is a firewall error until `isel-pic14e` exists.
* DFP/ATDF → TOML generator is a follow-up (#86); `.atdf` itself is never committed, only the TOML it generates (licence-clean, same posture as config transcription today).

## Revisit if

* `hal` needs SFR names that cannot be expressed in the optional `sfrs` table, or a new core requires fields not in the schema -- extend the schema, don't revert to Rust consts.
