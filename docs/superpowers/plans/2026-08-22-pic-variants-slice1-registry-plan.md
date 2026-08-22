# Slice 1 -- Device registry: file-per-device TOML + build.rs + --target

**Parent spec:** `docs/superpowers/specs/2026-08-22-pic-variants-design.md` (§4–§7)<br>
**Tickets:** `epic-cc#83` (primary), blocks `#84` `#85`<br>
**Scope:** groundwork only; no new device, no CI split, no DFP generator

## Goal

Prove "add a PIC = add a file" for the compiler. Two seed devices come from TOML via `build.rs`; `epic-cc --target <name>` resolves via `by_name`; `fsr_window` no longer hardcodes 4 windows.

## Steps

1. **TOML seeds** -- create `crates/device/devices/p16f877a.toml` and `p18f4550.toml` transcribed from current `PIC16F877A`/`PIC18F4550` consts. No new fields beyond spec §4 `sfrs = []`.
2. **Schema types** -- `crates/device/src/schema.rs` (private) with `DeviceToml`, `ConfigToml`, `FieldToml` etc., `serde` derives, `#[serde(deny_unknown_fields)]`.
3. **build.rs** -- `crates/device/build.rs`: glob `devices/*.toml`, `toml::from_str`, validate invariants (spec §4 list), emit `OUT_DIR/devices.rs` with `pub const` blocks + `ALL` + `by_name`. `rerun-if-changed=devices`.
4. **lib.rs switch** -- `src/lib.rs` `include!(concat!(env!("OUT_DIR"), "/devices.rs"))`; keep `impl Device`, `Fuse*`, `ConfigRegion`.
5. **De-hardcode** -- `crates/isel/src/lib.rs:fsr_window`: derive windows from `Device::ram_banks`/`common_ram`. Audit `crates/sim` map construction for same.
6. **Driver flag** -- `crates/driver/src/cli.rs` + `main.rs`: `--target`/`-mcu` + `PIC8_DEVICE` env, `by_name` lookup, unknown lists `ALL`.
7. **Tests** -- `crates/device/tests/device.rs`: schema round-trip, `by_name`, negative overlapping-banks; `crates/alloc` existing `PIC16F877A` refs become `by_name("p16f877a").unwrap()`.
8. **Docs** -- ADR-019 committed, spec committed, `docs/05-verification.md` note.

## Acceptance (from #83)

- `cargo test -p device` green, overlapping-banks negative panics with precise file:field.
- `cargo test` full green; `epic-cc --target p16f877a` vs `master` HEX identical for `add.c`.
- Unknown target error lists `p16f877a, p18f4550`.
- No hand-written Rust const remains in `lib.rs` (only `include!`).

## Not in this slice

CI split (#84), `p16f887` (#85), DFP generator (#86).
