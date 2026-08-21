# CC-3 Silicon-Real Codegen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `epic-cc` compiles a program that can boot real silicon: absolute placement (`EPIC_AT`), configuration words for both supported devices (`EPIC_CONFIG`), and an automatically-derived clock-frequency macro (`EPIC_FOSC_HZ`), all through a shipped `epic-cc.h` header.

**Architecture:** Per [D-8, D-9, D-10](../../31-ecosystem-integration-design.md). `EPIC_AT` wires the already-existing `ir::Global.addr` field through a section attribute. Config words are per-device `FuseField`/`ConfigRegion` data (ADR-004's device-as-data convention), resolved from an `EPIC_CONFIG("...")` string and emitted through a new multi-region HEX writer that leaves the existing single-region path untouched. `EPIC_FOSC_HZ` is a preprocessor macro resolved by a driver-side text pre-scan before clang ever runs, not a linker symbol and not a two-pass compile.

**Tech Stack:** Rust 1.97.1, Cargo workspace, no external crates. Docker dev image. `gpasm` 1.5.2 as the config-word cross-check oracle (invoked as a process only, per the GPL boundary). Both device datasheets, DS39582C (PIC16F87XA) and DS39632E (PIC18F2455/2550/4455/4550), vendored at `vendor/microchip/datasheets/` for this session; every field's bit position, mask, and encoding in this plan was read from those PDFs directly and cross-checked against `gpasm`'s independently-built device data, not reconstructed from memory.

**Spec:** [`docs/31-ecosystem-integration-design.md`](../../31-ecosystem-integration-design.md), decisions D-8, D-9, D-10.

## Global Constraints

- **Zero external crate dependencies**, same as CC-1.
- **The existing single-region `to_hex` is never modified.** A new `to_hex_regions` entry point sits beside it; every existing PIC14 fixture's golden output must stay byte-identical.
- **Erased-baseline rule, stated once, applied uniformly:** every config byte starts from its device's `erased_baseline` (confirmed empirically, see Task 2/3), and every field defaults to the datasheet's erased/unprogrammed encoding for that field, **except** the five items D-4 named as epic-cc's deliberate policy (watchdog off, low-voltage programming off, brown-out enabled, no code/write/table-read protection, debugger off) and the oscillator-tree fields, which have no default at all (`None`, override required).
- **`XINST` is `locked = Some("off")`.** An `EPIC_CONFIG` override attempting `xinst=on` panics naming the field and value. No other field in either device's bit set shares this hazard (confirmed by sweeping both tables against what `isel`/`isel-pic18` actually emit, `docs/31` D-9).
- **Panics are the error surface.** Unknown field, unknown value, missing required field, locked-field violation, more than one `EPIC_CONFIG` invocation in the whole program: all panic with the exact offending token(s) and, where applicable, the list of valid options.
- **No em-dashes**, Conventional Commits, hooks installed, same as CC-1.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/driver/src/epic_cc_h.rs` | **Create.** The `epic-cc.h` text, embedded as a Rust string constant and written to a temp include directory the driver adds to every clang invocation via `-I`. |
| `crates/device/src/lib.rs` | Gains `FuseValue`, `FuseField`, `ConfigRegion`, and a `config: ConfigRegion` field on `Device`; both `PIC16F877A` and `PIC18F4550` get real tables. |
| `crates/device/src/config.rs` | **Create.** `resolve_config(region: &ConfigRegion, spec: &str) -> Vec<u8>`, the `EPIC_CONFIG` string parser and resolver. Lives in `device` because it operates purely on `ConfigRegion` data, no driver or IR dependency. |
| `crates/device/tests/config.rs` | **Create.** Unit tests for parsing, defaults, `locked`, required-field panics. |
| `crates/asm/src/lib.rs` | Gains `to_hex_regions`. `to_hex` unmodified. |
| `crates/asm/tests/hex_regions.rs` | **Create.** Single-chunk parity with `to_hex`, two-chunk boundary crossing. |
| `crates/irparse/src/lib.rs` | Gains placement-attribute reading: `Global.addr` populated from a `.epicat.<hex>` section name instead of hardcoded `None`. |
| `crates/irparse/tests/placement.rs` | **Create.** Unit test for the section-name parse. |
| `crates/driver/src/prescan.rs` | **Create.** The `EPIC_CONFIG` raw-text pre-scanner: comment/string-literal-aware, finds zero or one top-level invocation across the input files. |
| `crates/driver/tests/prescan.rs` | **Create.** Unit tests for the scanner, including the comment/string-literal traps. |
| `crates/driver/src/main.rs` | Wires: writes `epic-cc.h` to a temp dir, adds it to `-I`; pre-scans for `EPIC_CONFIG`, resolves `EPIC_FOSC_HZ`, adds `-D`; after the merge, extracts the authoritative `EPIC_CONFIG` payload from the IR and cross-checks it against the pre-scan; resolves and emits config bytes via `to_hex_regions`; prints the resolved report. |
| `crates/driver/tests/fixtures/gpasm_config_*.asm` | **Create.** Hand-written `gpasm` inputs for the cross-check, one per device, using representative non-default field combinations. |
| `crates/driver/tests/config_e2e.rs` | **Create.** End-to-end: `EPIC_AT` + `EPIC_CONFIG` + `EPIC_FOSC_HZ` together, through the real driver, `gpasm`-cross-checked bytes at the right HEX address. |

Task order: Tasks 1, 2, 4, and 5 are independent of each other (do 1 and 2 first, matching CC-1's small-first pattern). Task 3 needs Task 2's data model and is the large PIC18 transcription. Task 6 needs 1, 2, 3, 4, and 5, all of it. Task 7 (the `gpasm` cross-check) needs 2 and 3. Task 8 (end-to-end acceptance) needs 6.

---

### Task 1: `EPIC_AT` placement

`ir::Global.addr` already exists (`crates/ir/src/lib.rs:137`) and `serialize` already prints it (`:213`); `irparse` just hardcodes `None` (`crates/irparse/src/lib.rs:1054`) because nothing feeds it. This closes that gap.

**Files:**
- Create: `crates/driver/src/epic_cc_h.rs`
- Modify: `crates/irparse/src/lib.rs`
- Create: `crates/irparse/tests/placement.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub const EPIC_CC_H: &str` (the header text). Task 6 writes it to a temp `-I` directory. `irparse::parse_ll` populates `Global.addr` whenever a global carries a `.epicat.<hex>` section.

- [ ] **Step 1: Write the failing test**

Create `crates/irparse/tests/placement.rs`:

```rust
use irparse::parse_ll;

#[test]
fn reads_the_placement_address_from_a_section_name() {
    let ll = "\
@port = dso_local global i8 0, section \".epicat.0x0F81\", align 1

define dso_local void @main() {
  ret void
}
";
    let m = parse_ll(ll);
    let g = m.globals.iter().find(|g| g.name == "port").unwrap();
    assert_eq!(g.addr, Some(0x0F81));
}

#[test]
fn leaves_addr_none_for_an_unplaced_global() {
    let ll = "\
@x = dso_local global i8 0, align 1

define dso_local void @main() {
  ret void
}
";
    let m = parse_ll(ll);
    let g = m.globals.iter().find(|g| g.name == "x").unwrap();
    assert_eq!(g.addr, None);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `make test CRATE=irparse`
Expected: FAIL, `reads_the_placement_address_from_a_section_name` gets `addr: None` instead of `Some(0x0F81)`.

- [ ] **Step 3: Implement**

Find the global-parsing block in `crates/irparse/src/lib.rs` (the one ending `globals.push(Global { name, ty, is_const, size, bytes, addr: None });` around line 1054). The raw global line, before `after`/`rest` splitting, is `line` (the full `@name = ... section "..." ...` text). Add, just before that `globals.push` call:

```rust
    // EPIC_AT(addr) expands to __attribute__((section(".epicat." #addr))); clang
    // forwards section attributes on globals verbatim (probed against the pinned
    // clang 20.1.8, docs/31 D-2/§5), so a placed global's raw .ll line contains
    // `section ".epicat.0x0F81"`. Everything else keeps addr: None.
    let addr = line
        .find("section \".epicat.")
        .map(|i| &line[i + "section \".epicat.".len()..])
        .and_then(|rest| rest.split('"').next())
        .map(|hex| {
            u16::from_str_radix(hex.trim_start_matches("0x"), 16)
                .unwrap_or_else(|_| panic!("irparse: bad EPIC_AT address {hex:?} on @{name}"))
        });
```

Then change the push to `globals.push(Global { name, ty, is_const, size, bytes, addr });`.

- [ ] **Step 4: Run to verify it passes**

Run: `make test CRATE=irparse`
Expected: PASS, both new tests, and every pre-existing `irparse` test still green (this touches only the `addr` field, which nothing else in the parser reads yet).

- [ ] **Step 5: Write `epic-cc.h`**

Create `crates/driver/src/epic_cc_h.rs`:

```rust
//! The header epic-cc ships to user code. Every macro reduces to
//! `__attribute__((section(...)))`, the one attribute form clang forwards
//! verbatim into the .ll (confirmed against the pinned clang 20.1.8,
//! docs/31 D-2/D-9/§5), so nothing here needs clang's cooperation beyond
//! that one already-probed fact.

pub const EPIC_CC_H: &str = r#"#ifndef EPIC_CC_H
#define EPIC_CC_H

/* Absolute placement: pins a global to a fixed address. epic-cc reads the
 * address back out of the section name; see irparse's EPIC_AT handling. */
#define EPIC_AT(addr) __attribute__((section(".epicat." #addr)))

/* Config words: exactly one EPIC_CONFIG(...) is permitted across the whole
 * program. epic-cc finds it two ways: a cheap raw-text pre-scan (to derive
 * EPIC_FOSC_HZ before clang runs) and, authoritatively, this section-tagged
 * dummy symbol after the whole program is merged. */
#define EPIC_CONFIG(spec) \
    static const char __epic_config[] __attribute__((used, section(".epiccfg." spec))) = spec

/* Derived from the resolved config words; see the driver's pre-scan. Not
 * usable as a link-time-only symbol on purpose: it must work in #if and in
 * a compile-time array bound, so it is a real preprocessor macro. */
#ifndef EPIC_FOSC_HZ
#define EPIC_FOSC_HZ 0
#endif

#endif /* EPIC_CC_H */
"#;
```

The `#ifndef EPIC_FOSC_HZ` guard matters: the driver always adds `-D EPIC_FOSC_HZ=<value>` (Task 6), so this fallback only fires if a `.c` file is compiled without going through the driver's normal path (a raw `clang -I <header dir>` invocation for testing, for instance); `0` there is a deliberately inert placeholder, not a claim about any real device.

- [ ] **Step 6: Add a crate-level module for it**

`crates/driver/src/epic_cc_h.rs` is a plain data module; wire it into `crates/driver/src/lib.rs`:

```rust
pub mod clang_discovery;
pub mod cli;
pub mod epic_cc_h;
```

(Task 6 adds `pub mod prescan;` here too; don't add it yet, Task 5 owns that file.)

- [ ] **Step 7: Commit**

```bash
git add crates/irparse/src/lib.rs crates/irparse/tests/placement.rs crates/driver/src/epic_cc_h.rs crates/driver/src/lib.rs
git commit -m "feat(irparse): read EPIC_AT placement from section names"
```

---

### Task 2: config data model + PIC16F877A's real table

The full model, plus the smaller of the two devices' tables, to prove the model before scaling to PIC18F4550.

**Files:**
- Modify: `crates/device/src/lib.rs`
- Create: `crates/device/src/config.rs`
- Create: `crates/device/tests/config.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub struct FuseValue`, `pub struct FuseField`, `pub struct ConfigRegion`, `Device.config: ConfigRegion`, `pub fn resolve_config(region: &ConfigRegion, spec: &str) -> Vec<u8>`. Task 4 (PIC18 table), Task 6 (driver wiring), and Task 7 (pre-scan) all consume `resolve_config` and the two devices' `config` fields.

- [ ] **Step 1: Write the failing tests**

Create `crates/device/tests/config.rs`. These exercise the resolver against `PIC16F877A::config` directly, so they double as acceptance tests for the real table:

```rust
use device::{resolve_config, PIC16F877A};

#[test]
fn erased_baseline_is_the_datasheet_stated_value() {
    // DS39582C Register 14-1, note 1: "the erased (unprogrammed) value of
    // the Configuration Word is 3FFFh" (low byte 0xFF, high byte 0x3F).
    assert_eq!(PIC16F877A.config.erased_baseline, &[0xFF, 0x3F]);
}

#[test]
fn resolves_a_representative_override_and_matches_hand_computation() {
    // osc=xt, wdt=off, pwrt=on, bor=on, lvp=off, cpd=off, wrt=off,
    // debug=off, cp=off. Hand-computed against DS39582C Register 14-1 and
    // cross-checked against gpasm 1.5.2 (2026-08-21): word 0x3F71.
    let bytes = resolve_config(
        &PIC16F877A.config,
        "osc=xt, wdt=off, pwrt=on, bor=on, lvp=off, cpd=off, wrt=off, debug=off, cp=off",
    );
    assert_eq!(bytes, vec![0x71, 0x3F]);
}

#[test]
#[should_panic(expected = "field 'osc' has no default")]
fn panics_when_the_required_oscillator_field_is_missing() {
    resolve_config(&PIC16F877A.config, "wdt=off");
}

#[test]
#[should_panic(expected = "unknown field 'wat'")]
fn panics_on_an_unknown_field() {
    resolve_config(&PIC16F877A.config, "osc=xt, wat=off");
}

#[test]
#[should_panic(expected = "unknown value 'turbo' for field 'osc'")]
fn panics_on_an_unknown_value() {
    resolve_config(&PIC16F877A.config, "osc=turbo");
}

#[test]
fn unmentioned_fields_take_their_default() {
    // Only osc set (required); everything else should resolve to its
    // stated default, matching the full-override test's non-osc bytes
    // exactly, since PIC16F877A.config's defaults ARE that combination.
    let bytes = resolve_config(&PIC16F877A.config, "osc=xt");
    assert_eq!(bytes, vec![0x71, 0x3F]);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `make test CRATE=device`
Expected: FAIL, `unresolved import device::resolve_config` (and `PIC16F877A.config` does not exist yet).

- [ ] **Step 3: Implement the data model**

Append to `crates/device/src/lib.rs` (after the existing `impl Device` block):

```rust
#[derive(Clone, Copy, Debug)]
pub struct FuseValue {
    pub name: &'static str,
    /// The raw bit pattern this value encodes, already positioned at bit 0
    /// (shifted into place by `resolve_config` using the field's `shift`).
    pub bits: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct FuseField {
    pub name: &'static str,
    /// Offset into the region, 0-based (e.g. CONFIG4L is offset 6 in the
    /// PIC18F4550's region, which starts at `base_byte_addr`).
    pub byte_offset: u16,
    pub mask: u8,
    pub shift: u8,
    pub values: &'static [FuseValue],
    /// `None`: no safe default exists (oscillator-tree fields); an
    /// EPIC_CONFIG override is required, or resolution panics.
    pub default: Option<&'static str>,
    /// `Some(name)`: the only value epic-cc's backend can honor. An
    /// override to anything else panics. Used for `XINST` only.
    pub locked: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
pub struct ConfigRegion {
    pub base_byte_addr: u32,
    pub num_bytes: u16,
    /// Raw flash content before any FuseField is applied. Confirmed
    /// per-device, not assumed uniform; see docs/31 D-9.
    pub erased_baseline: &'static [u8],
    pub fields: &'static [FuseField],
}
```

Add `pub config: ConfigRegion` to `Device`, and add it to both `PIC16F877A` (Task 2) and a placeholder `PIC18F4550` (Task 3 fills it in for real; for now, an empty region keeps the crate compiling):

```rust
pub const PIC16F877A: Device = Device {
    // ...existing fields unchanged...
    config: ConfigRegion {
        base_byte_addr: 0x400E,
        num_bytes: 2,
        // DS39582C Register 14-1, note 1: erased value of the
        // Configuration Word is 3FFFh (low byte 0xFF, high byte 0x3F).
        erased_baseline: &[0xFF, 0x3F],
        fields: &[
            // byte_offset 0 (word bits 7:0)
            FuseField {
                name: "osc", byte_offset: 0, mask: 0x03, shift: 0,
                values: &[
                    FuseValue { name: "rc", bits: 0b11 },
                    FuseValue { name: "hs", bits: 0b10 },
                    FuseValue { name: "xt", bits: 0b01 },
                    FuseValue { name: "lp", bits: 0b00 },
                ],
                default: None, locked: None,
            },
            FuseField {
                name: "wdt", byte_offset: 0, mask: 0x04, shift: 2,
                values: &[
                    FuseValue { name: "on", bits: 1 },
                    FuseValue { name: "off", bits: 0 },
                ],
                default: Some("off"), locked: None, // D-4 policy
            },
            FuseField {
                // PWRTEN: 1 = PWRT disabled, 0 = PWRT enabled (inverted).
                name: "pwrt", byte_offset: 0, mask: 0x08, shift: 3,
                values: &[
                    FuseValue { name: "on", bits: 0 },
                    FuseValue { name: "off", bits: 1 },
                ],
                default: Some("on"), locked: None, // D-4 policy: PWRT on
            },
            FuseField {
                name: "bor", byte_offset: 0, mask: 0x40, shift: 6,
                values: &[
                    FuseValue { name: "on", bits: 1 },
                    FuseValue { name: "off", bits: 0 },
                ],
                default: Some("on"), locked: None, // D-4 policy
            },
            FuseField {
                name: "lvp", byte_offset: 0, mask: 0x80, shift: 7,
                values: &[
                    FuseValue { name: "on", bits: 1 },
                    FuseValue { name: "off", bits: 0 },
                ],
                default: Some("off"), locked: None, // D-4 policy
            },
            // byte_offset 1 (word bits 13:8; bits 15:14 do not exist,
            // the word is 14 bits, so byte 1 has no field past bit 5)
            FuseField {
                name: "cpd", byte_offset: 1, mask: 0x01, shift: 0,
                values: &[
                    FuseValue { name: "off", bits: 1 },
                    FuseValue { name: "on", bits: 0 },
                ],
                default: Some("off"), locked: None, // D-4 policy
            },
            FuseField {
                // WRT1:WRT0, PIC16F876A/877A decode (DS39582C Reg 14-1).
                name: "wrt", byte_offset: 1, mask: 0x06, shift: 1,
                values: &[
                    FuseValue { name: "off", bits: 0b11 },
                    FuseValue { name: "protect_0000_00ff", bits: 0b10 },
                    FuseValue { name: "protect_0000_07ff", bits: 0b01 },
                    FuseValue { name: "protect_0000_0fff", bits: 0b00 },
                ],
                default: Some("off"), locked: None, // D-4 policy
            },
            FuseField {
                name: "debug", byte_offset: 1, mask: 0x08, shift: 3,
                values: &[
                    FuseValue { name: "off", bits: 1 },
                    FuseValue { name: "on", bits: 0 },
                ],
                default: Some("off"), locked: None, // D-4 policy
            },
            FuseField {
                name: "cp", byte_offset: 1, mask: 0x20, shift: 5,
                values: &[
                    FuseValue { name: "off", bits: 1 },
                    FuseValue { name: "on", bits: 0 },
                ],
                default: Some("off"), locked: None, // D-4 policy
            },
        ],
    },
    // ...
};
```

**Do not paraphrase these values.** Every `mask`/`shift`/`bits` pair above comes directly from DS39582C Register 14-1 (transcribed 2026-08-21) and the whole-word combination is cross-checked against `gpasm` 1.5.2 (see Step 1's test). If a future editor changes one, they must re-run the `gpasm` cross-check, not just eyeball it.

- [ ] **Step 4: Implement `resolve_config`**

Create `crates/device/src/config.rs`:

```rust
//! `EPIC_CONFIG("...")` string parsing and resolution against a device's
//! `ConfigRegion`. Pure data in, `Vec<u8>` out: no IR, no driver dependency.

use crate::ConfigRegion;

/// Resolve a comma-separated `key=value, key=value` spec against `region`,
/// starting from `region.erased_baseline` and applying each mentioned
/// field, each unmentioned field's default, in that order.
///
/// Panics if: a required field (`default: None`) is never mentioned; a
/// mentioned field name does not exist in `region`; a mentioned value name
/// does not exist for that field; a field is `locked` to a different value
/// than the one given.
pub fn resolve_config(region: &ConfigRegion, spec: &str) -> Vec<u8> {
    let mut bytes = region.erased_baseline.to_vec();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for pair in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (key, val) = pair
            .split_once('=')
            .unwrap_or_else(|| panic!("device: malformed EPIC_CONFIG entry {pair:?} (expected key=value)"));
        let (key, val) = (key.trim(), val.trim());

        let field = region
            .fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(key))
            .unwrap_or_else(|| panic!("device: unknown field {key:?} in EPIC_CONFIG"));

        if let Some(only) = field.locked {
            if !val.eq_ignore_ascii_case(only) {
                panic!(
                    "device: field {:?} is locked to {only:?} (epic-cc's backend cannot honor \
                     other values); got {val:?}",
                    field.name
                );
            }
        }

        let fv = field
            .values
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(val))
            .unwrap_or_else(|| {
                let opts: Vec<&str> = field.values.iter().map(|v| v.name).collect();
                panic!(
                    "device: unknown value {val:?} for field {:?}, expected one of {opts:?}",
                    field.name
                )
            });

        apply(&mut bytes, field, fv.bits);
        seen.insert(field.name);
    }

    for field in region.fields {
        if seen.contains(field.name) {
            continue;
        }
        let default_name = field.default.unwrap_or_else(|| {
            panic!(
                "device: field {:?} has no default and was not set by EPIC_CONFIG; \
                 this device cannot boot without an explicit value. Valid values: {:?}",
                field.name,
                field.values.iter().map(|v| v.name).collect::<Vec<_>>()
            )
        });
        let fv = field
            .values
            .iter()
            .find(|v| v.name == default_name)
            .unwrap_or_else(|| panic!("device: field {:?}'s own default {default_name:?} is not one of its values (data bug)", field.name));
        apply(&mut bytes, field, fv.bits);
    }

    bytes
}

fn apply(bytes: &mut [u8], field: &crate::FuseField, bits: u8) {
    let i = field.byte_offset as usize;
    bytes[i] = (bytes[i] & !field.mask) | ((bits << field.shift) & field.mask);
}
```

- [ ] **Step 5: Wire the module and the `Device.config` field**

In `crates/device/src/lib.rs`, add `mod config; pub use config::resolve_config;` near the top, and add `pub config: ConfigRegion` to the `Device` struct (Step 3 already shows `PIC16F877A`'s literal; `PIC18F4550` gets a placeholder empty region for now so the crate compiles: `config: ConfigRegion { base_byte_addr: 0x300000, num_bytes: 0, erased_baseline: &[], fields: &[] }`, replaced for real in Task 3).

- [ ] **Step 6: Run to verify it passes**

Run: `make test CRATE=device`
Expected: PASS, all six new tests. `resolves_a_representative_override_and_matches_hand_computation` and `unmentioned_fields_take_their_default` both assert `[0x71, 0x3F]`, the exact bytes `gpasm` produced for the equivalent `__CONFIG` directive (verified 2026-08-21, see docs/31 D-9).

- [ ] **Step 7: Commit**

```bash
git add crates/device/src/lib.rs crates/device/src/config.rs crates/device/tests/config.rs
git commit -m "feat(device): config-word data model and PIC16F877A's real table"
```

---

### Task 3: PIC18F4550's real table

The large transcription task: 39 fields across 14 byte addresses (12 real registers, 2 gaps), from DS39632E §25.1, Table 25-1 and Registers 25-1 through 25-12.

**Files:**
- Modify: `crates/device/src/lib.rs` (replace `PIC18F4550`'s placeholder `config` field)
- Modify: `crates/device/tests/config.rs` (add PIC18-targeted tests)

**Interfaces:**
- Consumes: `FuseField`/`FuseValue`/`ConfigRegion`/`resolve_config` from Task 2.
- Produces: `PIC18F4550.config`, fully populated. Task 4 and Task 6 consume it; Task 8's `gpasm` cross-check validates it independently.

- [ ] **Step 1: Write the failing tests**

Append to `crates/device/tests/config.rs`:

```rust
use device::PIC18F4550;

#[test]
fn pic18_erased_baseline_is_all_ff_confirmed_against_gpasm() {
    // Confirmed empirically 2026-08-21: assembling CONFIG4L through gpasm
    // 1.5.2 with every named field set left both genuinely-unimplemented
    // bits AND the untouched gap byte 0x300007 at 0xFF, not the "reads as
    // 0" value DS39632E's register legends state (that describes SFR
    // read-time masking, not what gets written to flash).
    assert_eq!(PIC18F4550.config.erased_baseline, &[0xFF; 14]);
    assert_eq!(PIC18F4550.config.num_bytes, 14);
    assert_eq!(PIC18F4550.config.base_byte_addr, 0x300000);
}

#[test]
fn xinst_is_locked_off() {
    let f = PIC18F4550
        .config
        .fields
        .iter()
        .find(|f| f.name == "xinst")
        .unwrap();
    assert_eq!(f.locked, Some("off"));
}

#[test]
#[should_panic(expected = "field 'xinst' is locked to \"off\"")]
fn overriding_xinst_on_panics() {
    resolve_config(
        &PIC18F4550.config,
        "osc=hspll, plldiv=div5, cpudiv=div1, usbdiv=on, xinst=on",
    );
}

#[test]
fn resolves_config4l_and_matches_gpasm() {
    // gpasm 1.5.2, 2026-08-21: __CONFIG _CONFIG4L, _DEBUG_OFF_4L &
    // _XINST_OFF_4L & _ICPRT_OFF_4L & _LVP_OFF_4L & _STVREN_ON_4L -> 0x9B
    // at byte offset 6 (CONFIG4L, address 0x300006).
    let bytes = resolve_config(
        &PIC18F4550.config,
        "osc=hspll, plldiv=div5, cpudiv=div1, usbdiv=on, \
         debug=off, xinst=off, icprt=off, lvp=off, stvren=on",
    );
    assert_eq!(bytes[6], 0x9B);
    // The gap byte right after it stays at the erased baseline: gpasm's
    // own output for this test showed 0x300007 = 0xFF, untouched.
    assert_eq!(bytes[7], 0xFF);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `make test CRATE=device`
Expected: FAIL. `PIC18F4550.config` is still the Task 2 placeholder (`num_bytes: 0`, empty `fields`), so every assertion above fails and the `xinst` lookup panics with "not found" rather than the expected message.

- [ ] **Step 3: Implement the full table**

Replace `PIC18F4550`'s `config` placeholder in `crates/device/src/lib.rs` with the real table. Byte offsets 0-13 correspond to addresses `0x300000`-`0x30000D`; offsets 4 and 7 are gaps (DS39632E Table 25-1 lists no register there) and carry no `FuseField`.

```rust
config: ConfigRegion {
    base_byte_addr: 0x300000,
    num_bytes: 14,
    // Confirmed against gpasm 1.5.2, 2026-08-21 (see the crate's config.rs
    // tests): every byte in this region, including the two gap addresses,
    // erases to 0xFF. DS39632E's per-register "Default/Unprogrammed Value"
    // column in Table 25-1 describes the value read back through the SFR
    // (unimplemented bits forced to 0 by hardware), not the raw flash
    // content, which is what this array represents.
    erased_baseline: &[0xFF; 14],
    fields: &[
        // ---- offset 0: CONFIG1L (0x300000), DS39632E Register 25-1 ----
        FuseField {
            name: "usbdiv", byte_offset: 0, mask: 0x20, shift: 5,
            values: &[FuseValue { name: "on", bits: 1 }, FuseValue { name: "off", bits: 0 }],
            default: None, locked: None, // part of the clock tree
        },
        FuseField {
            name: "cpudiv", byte_offset: 0, mask: 0x18, shift: 3,
            values: &[
                FuseValue { name: "div1", bits: 0b00 },
                FuseValue { name: "div2", bits: 0b01 },
                FuseValue { name: "div3", bits: 0b10 },
                FuseValue { name: "div4", bits: 0b11 },
            ],
            default: None, locked: None,
        },
        FuseField {
            name: "plldiv", byte_offset: 0, mask: 0x07, shift: 0,
            values: &[
                FuseValue { name: "noprescale", bits: 0b000 }, // 4 MHz direct
                FuseValue { name: "div2", bits: 0b001 },       // 8 MHz input
                FuseValue { name: "div3", bits: 0b010 },       // 12 MHz input
                FuseValue { name: "div4", bits: 0b011 },       // 16 MHz input
                FuseValue { name: "div5", bits: 0b100 },       // 20 MHz input
                FuseValue { name: "div6", bits: 0b101 },       // 24 MHz input
                FuseValue { name: "div10", bits: 0b110 },      // 40 MHz input
                FuseValue { name: "div12", bits: 0b111 },      // 48 MHz input
            ],
            default: None, locked: None,
        },
        // ---- offset 1: CONFIG1H (0x300001), Register 25-2 ----
        FuseField {
            name: "ieso", byte_offset: 1, mask: 0x80, shift: 7,
            values: &[FuseValue { name: "on", bits: 1 }, FuseValue { name: "off", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "fcmen", byte_offset: 1, mask: 0x40, shift: 6,
            values: &[FuseValue { name: "on", bits: 1 }, FuseValue { name: "off", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            // Canonical (x=0) encoding for each named mode; DS39632E lists
            // some patterns as "111x"/"110x"/etc. where the low bit is a
            // don't-care. FOSC3:0.
            name: "osc", byte_offset: 1, mask: 0x0F, shift: 0,
            values: &[
                FuseValue { name: "hspll", bits: 0b1110 },
                FuseValue { name: "hs", bits: 0b1100 },
                FuseValue { name: "inths", bits: 0b1011 },
                FuseValue { name: "intxt", bits: 0b1010 },
                FuseValue { name: "intcko", bits: 0b1001 },
                FuseValue { name: "intio", bits: 0b1000 },
                FuseValue { name: "ecpll", bits: 0b0111 },
                FuseValue { name: "ecpio", bits: 0b0110 },
                FuseValue { name: "ec", bits: 0b0101 },
                FuseValue { name: "ecio", bits: 0b0100 },
                FuseValue { name: "xtpll", bits: 0b0010 },
                FuseValue { name: "xt", bits: 0b0000 },
            ],
            default: None, locked: None, // the oscillator field
        },
        // ---- offset 2: CONFIG2L (0x300002), Register 25-3 ----
        FuseField {
            name: "vregen", byte_offset: 2, mask: 0x20, shift: 5,
            values: &[FuseValue { name: "on", bits: 1 }, FuseValue { name: "off", bits: 0 }],
            default: Some("off"), locked: None, // matches erased default
        },
        FuseField {
            name: "borv", byte_offset: 2, mask: 0x18, shift: 3,
            values: &[
                FuseValue { name: "minimum", bits: 0b11 },
                FuseValue { name: "low", bits: 0b10 },
                FuseValue { name: "mid", bits: 0b01 },
                FuseValue { name: "maximum", bits: 0b00 },
            ],
            default: Some("minimum"), locked: None, // matches erased default
        },
        FuseField {
            name: "boren", byte_offset: 2, mask: 0x06, shift: 1,
            values: &[
                FuseValue { name: "hw_always", bits: 0b11 },
                FuseValue { name: "hw_off_in_sleep", bits: 0b10 },
                FuseValue { name: "sw", bits: 0b01 },
                FuseValue { name: "off", bits: 0b00 },
            ],
            default: Some("hw_always"), locked: None, // D-4 policy: BOR on
        },
        FuseField {
            // 1 = PWRT disabled, 0 = PWRT enabled (inverted, matches the
            // PIC16F877A's PWRTEN convention).
            name: "pwrt", byte_offset: 2, mask: 0x01, shift: 0,
            values: &[FuseValue { name: "on", bits: 0 }, FuseValue { name: "off", bits: 1 }],
            default: Some("on"), locked: None, // D-4 policy
        },
        // ---- offset 3: CONFIG2H (0x300003), Register 25-4 ----
        FuseField {
            name: "wdtps", byte_offset: 3, mask: 0x1E, shift: 1,
            values: &[
                FuseValue { name: "div1", bits: 0b0000 },
                FuseValue { name: "div2", bits: 0b0001 },
                FuseValue { name: "div4", bits: 0b0010 },
                FuseValue { name: "div8", bits: 0b0011 },
                FuseValue { name: "div16", bits: 0b0100 },
                FuseValue { name: "div32", bits: 0b0101 },
                FuseValue { name: "div64", bits: 0b0110 },
                FuseValue { name: "div128", bits: 0b0111 },
                FuseValue { name: "div256", bits: 0b1000 },
                FuseValue { name: "div512", bits: 0b1001 },
                FuseValue { name: "div1024", bits: 0b1010 },
                FuseValue { name: "div2048", bits: 0b1011 },
                FuseValue { name: "div4096", bits: 0b1100 },
                FuseValue { name: "div8192", bits: 0b1101 },
                FuseValue { name: "div16384", bits: 0b1110 },
                FuseValue { name: "div32768", bits: 0b1111 },
            ],
            default: Some("div32768"), locked: None, // matches erased default
        },
        FuseField {
            name: "wdt", byte_offset: 3, mask: 0x01, shift: 0,
            values: &[FuseValue { name: "on", bits: 1 }, FuseValue { name: "off", bits: 0 }],
            default: Some("off"), locked: None, // D-4 policy
        },
        // offset 4: gap (0x300004), no register, no fields
        // ---- offset 5: CONFIG3H (0x300005), Register 25-5 ----
        FuseField {
            name: "mclre", byte_offset: 5, mask: 0x80, shift: 7,
            values: &[FuseValue { name: "on", bits: 1 }, FuseValue { name: "off", bits: 0 }],
            default: Some("on"), locked: None, // matches erased default
        },
        FuseField {
            name: "lpt1osc", byte_offset: 5, mask: 0x04, shift: 2,
            values: &[FuseValue { name: "on", bits: 1 }, FuseValue { name: "off", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "pbaden", byte_offset: 5, mask: 0x02, shift: 1,
            values: &[FuseValue { name: "on", bits: 1 }, FuseValue { name: "off", bits: 0 }],
            default: Some("on"), locked: None, // matches erased default
        },
        FuseField {
            name: "ccp2mx", byte_offset: 5, mask: 0x01, shift: 0,
            values: &[FuseValue { name: "rc1", bits: 1 }, FuseValue { name: "rb3", bits: 0 }],
            default: Some("rc1"), locked: None, // matches erased default
        },
        // ---- offset 6: CONFIG4L (0x300006), Register 25-6 ----
        FuseField {
            name: "debug", byte_offset: 6, mask: 0x80, shift: 7,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None, // D-4 policy
        },
        FuseField {
            name: "xinst", byte_offset: 6, mask: 0x40, shift: 6,
            values: &[FuseValue { name: "off", bits: 0 }, FuseValue { name: "on", bits: 1 }],
            default: Some("off"), locked: Some("off"), // codegen hazard, docs/31 D-9
        },
        FuseField {
            // DS39632E note: "Always leave this bit clear in all other
            // devices" (44-pin TQFP only). epic-cc does not model package
            // variants, so this is off unconditionally.
            name: "icprt", byte_offset: 6, mask: 0x20, shift: 5,
            values: &[FuseValue { name: "on", bits: 1 }, FuseValue { name: "off", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "lvp", byte_offset: 6, mask: 0x04, shift: 2,
            values: &[FuseValue { name: "on", bits: 1 }, FuseValue { name: "off", bits: 0 }],
            default: Some("off"), locked: None, // D-4 policy
        },
        FuseField {
            name: "stvren", byte_offset: 6, mask: 0x01, shift: 0,
            values: &[FuseValue { name: "on", bits: 1 }, FuseValue { name: "off", bits: 0 }],
            default: Some("on"), locked: None, // matches erased default
        },
        // offset 7: gap (0x300007), no register, no fields
        // ---- offset 8: CONFIG5L (0x300008), Register 25-7 ----
        FuseField {
            name: "cp0", byte_offset: 8, mask: 0x01, shift: 0,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None, // D-4 policy
        },
        FuseField {
            name: "cp1", byte_offset: 8, mask: 0x02, shift: 1,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "cp2", byte_offset: 8, mask: 0x04, shift: 2,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "cp3", byte_offset: 8, mask: 0x08, shift: 3,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        // ---- offset 9: CONFIG5H (0x300009), Register 25-8 ----
        FuseField {
            name: "cpd", byte_offset: 9, mask: 0x80, shift: 7,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "cpb", byte_offset: 9, mask: 0x40, shift: 6,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        // ---- offset 10: CONFIG6L (0x30000A), Register 25-9 ----
        FuseField {
            name: "wrt0", byte_offset: 10, mask: 0x01, shift: 0,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "wrt1", byte_offset: 10, mask: 0x02, shift: 1,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "wrt2", byte_offset: 10, mask: 0x04, shift: 2,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "wrt3", byte_offset: 10, mask: 0x08, shift: 3,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        // ---- offset 11: CONFIG6H (0x30000B), Register 25-10 ----
        FuseField {
            name: "wrtc", byte_offset: 11, mask: 0x20, shift: 5,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "wrtb", byte_offset: 11, mask: 0x40, shift: 6,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "wrtd", byte_offset: 11, mask: 0x80, shift: 7,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        // ---- offset 12: CONFIG7L (0x30000C), Register 25-11 ----
        FuseField {
            name: "ebtr0", byte_offset: 12, mask: 0x01, shift: 0,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "ebtr1", byte_offset: 12, mask: 0x02, shift: 1,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "ebtr2", byte_offset: 12, mask: 0x04, shift: 2,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        FuseField {
            name: "ebtr3", byte_offset: 12, mask: 0x08, shift: 3,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
        // ---- offset 13: CONFIG7H (0x30000D), Register 25-12 ----
        FuseField {
            name: "ebtrb", byte_offset: 13, mask: 0x40, shift: 6,
            values: &[FuseValue { name: "off", bits: 1 }, FuseValue { name: "on", bits: 0 }],
            default: Some("off"), locked: None,
        },
    ],
},
```

**Do not paraphrase these values either.** Every register above is cited to its DS39632E Register number; `CONFIG4L`'s combination is independently cross-checked against `gpasm` in Step 1's test.

- [ ] **Step 4: Run to verify it passes**

Run: `make test CRATE=device`
Expected: PASS, all tests including the new PIC18-targeted ones. `resolves_config4l_and_matches_gpasm` is the load-bearing one: if it fails, a transcription error exists somewhere in `CONFIG4L`'s five fields, and it must be fixed by re-reading DS39632E Register 25-6, not by adjusting the test's expected byte.

- [ ] **Step 5: Commit**

```bash
git add crates/device/src/lib.rs crates/device/tests/config.rs
git commit -m "feat(device): PIC18F4550's full config-word table"
```

---

### Task 4: multi-region HEX emission

**Files:**
- Modify: `crates/asm/src/lib.rs`
- Create: `crates/asm/tests/hex_regions.rs`

**Interfaces:**
- Consumes: nothing new (operates on `&[u16]` chunks, same shape `to_hex` already takes).
- Produces: `pub fn to_hex_regions(chunks: &[(u32, &[u16])]) -> String`. Task 6 calls it for PIC18's two-chunk case; PIC14's config word instead reuses the existing single-array `to_hex` path (Task 6 explains why).

- [ ] **Step 1: Write the failing tests**

Create `crates/asm/tests/hex_regions.rs`:

```rust
use asm::{to_hex, to_hex_regions};

#[test]
fn a_single_chunk_at_zero_matches_to_hex_exactly() {
    let words = vec![0x2830u16, 0x0064, 0x0000];
    assert_eq!(to_hex_regions(&[(0, &words)]), to_hex(&words));
}

#[test]
fn two_chunks_crossing_a_64k_boundary_emit_a_second_extended_address_record() {
    // Chunk 1: one word at byte address 0. Chunk 2: one word at byte
    // address 0x300000 (word address 0x180000), PIC18F4550's config
    // region base. The upper 16 bits differ (0x0000 vs 0x0030), so a
    // second :04 record must appear before the second chunk's data.
    let a = vec![0x1234u16];
    let b = vec![0x9BFFu16];
    let hex = to_hex_regions(&[(0, &a), (0x300000, &b)]);
    let lines: Vec<&str> = hex.lines().collect();
    assert_eq!(lines[0], ":020000040000FA"); // upper=0x0000
    assert!(lines.iter().any(|l| *l == ":020000040030CA")); // upper=0x0030
    // The config word's own data record: 2 bytes, address 0x0000 within
    // the 0x0030 window, low byte 0xFF, high byte 0x9B.
    assert!(hex.contains(":02000000FF9B"));
    assert_eq!(lines.last().unwrap(), &":00000001FF");
}

#[test]
fn chunks_at_the_same_upper_16_bits_share_one_extended_address_record() {
    let a = vec![0x1111u16];
    let b = vec![0x2222u16];
    let hex = to_hex_regions(&[(0, &a), (0x10, &b)]);
    let extended_records = hex.lines().filter(|l| l.ends_with("040000FA")).count();
    assert_eq!(extended_records, 1);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `make test CRATE=asm`
Expected: FAIL, `cannot find function 'to_hex_regions'`.

- [ ] **Step 3: Implement**

Append to `crates/asm/src/lib.rs`, near `to_hex` (`:561`). This factors the record-writing loop `to_hex` already has into a shared helper, then adds the multi-region entry point; `to_hex` itself is not touched:

```rust
/// Multi-region Intel HEX: each `(base_byte_addr, words)` chunk is written
/// in order, with a new `:04` extended-linear-address record emitted only
/// when a chunk's upper 16 address bits differ from the previous one. A
/// single chunk at base 0 produces output byte-identical to `to_hex`.
///
/// Unlike `to_hex`, trailing zero words within a chunk are NOT trimmed:
/// config-word chunks are small and every byte (including an erased 0xFF)
/// is meaningful, unlike a program image's unused flash tail.
pub fn to_hex_regions(chunks: &[(u32, &[u16])]) -> String {
    let mut hex = String::new();
    let mut current_upper: Option<u32> = None;
    for &(base_byte_addr, words) in chunks {
        let upper = base_byte_addr >> 16;
        if current_upper != Some(upper) {
            let rec = [0x02, 0x00, 0x00, 0x04, (upper >> 8) as u8, (upper & 0xFF) as u8];
            hex.push_str(&hex_record(&rec));
            current_upper = Some(upper);
        }
        let mut addr = 0usize;
        while addr < words.len() {
            let n = (words.len() - addr).min(8);
            let mut body = vec![0u8; 2 * n];
            for (i, w) in words[addr..addr + n].iter().enumerate() {
                body[2 * i] = (w & 0xFF) as u8;
                body[2 * i + 1] = ((w >> 8) & 0xFF) as u8;
            }
            let byte_addr = (base_byte_addr as usize & 0xFFFF) + addr * 2;
            let mut rec = vec![(2 * n) as u8, (byte_addr >> 8) as u8, (byte_addr & 0xFF) as u8, 0x00];
            rec.extend_from_slice(&body);
            hex.push_str(&hex_record(&rec));
            addr += n;
        }
    }
    hex.push_str(":00000001FF\n");
    hex
}

/// Render one Intel HEX record (byte count/address/type already in `rec`,
/// data appended) with its checksum, `:`-prefixed, newline-terminated.
fn hex_record(rec: &[u8]) -> String {
    let sum: u16 = rec.iter().map(|&b| b as u16).sum();
    let checksum = (0x100 - (sum & 0xFF)) as u8;
    let mut s = String::from(":");
    for b in rec {
        s.push_str(&format!("{b:02X}"));
    }
    s.push_str(&format!("{checksum:02X}\n"));
    s
}
```

**Check the single-chunk parity test carefully before trusting it.** `to_hex` trims trailing zero words and always starts its one extended-address record at `upper=0`; `to_hex_regions`'s single-chunk case does neither (no trimming, and it only writes the record because `(0, &words)`'s upper happens to be 0). The test as written uses non-zero trailing data (`0x0000` as the last word is fine since `to_hex`'s trim only drops words *after* the last non-zero one, and there is none here) so it should pass unmodified, but if it does not, the mismatch is real and must be resolved by matching `to_hex`'s exact behavior for the zero-base case, not by weakening the test.

- [ ] **Step 4: Run to verify it passes**

Run: `make test CRATE=asm`
Expected: PASS, all three new tests, and every pre-existing `asm` test (including the byte-level PIC14 and PIC18 instruction encoding tests) untouched, since `to_hex` itself was not modified.

- [ ] **Step 5: Commit**

```bash
git add crates/asm/src/lib.rs crates/asm/tests/hex_regions.rs
git commit -m "feat(asm): multi-region Intel HEX emission for config words"
```

---

### Task 5: `EPIC_FOSC_HZ` pre-scan

**Files:**
- Create: `crates/driver/src/prescan.rs`
- Create: `crates/driver/tests/prescan.rs`

**Interfaces:**
- Consumes: raw file contents (no clang, no IR).
- Produces: `pub fn find_epic_config(sources: &[(String, String)]) -> Option<String>` (filename+contents pairs in, the spec string out, or `None`). Panics if more than one unconditional top-level invocation is found across all files. Task 6 calls it before invoking clang, and again (via the same function, on the post-merge canonical source, see Task 6) to cross-check.

- [ ] **Step 1: Write the failing tests**

Create `crates/driver/tests/prescan.rs`:

```rust
use driver::prescan::find_epic_config;

fn src(s: &[(&str, &str)]) -> Vec<(String, String)> {
    s.iter().map(|(n, c)| (n.to_string(), c.to_string())).collect()
}

#[test]
fn finds_a_simple_invocation() {
    let found = find_epic_config(&src(&[("main.c", "EPIC_CONFIG(\"osc=hspll, wdt=off\");\n")]));
    assert_eq!(found.as_deref(), Some("osc=hspll, wdt=off"));
}

#[test]
fn returns_none_when_absent() {
    assert_eq!(find_epic_config(&src(&[("main.c", "void main(void) {}\n")])), None);
}

#[test]
fn skips_a_line_comment_that_looks_like_an_invocation() {
    let found = find_epic_config(&src(&[(
        "main.c",
        "// EPIC_CONFIG(\"osc=xt\");\nEPIC_CONFIG(\"osc=hspll\");\n",
    )]));
    assert_eq!(found.as_deref(), Some("osc=hspll"));
}

#[test]
fn skips_a_block_comment_that_looks_like_an_invocation() {
    let found = find_epic_config(&src(&[(
        "main.c",
        "/* EPIC_CONFIG(\"osc=xt\"); */\nEPIC_CONFIG(\"osc=hspll\");\n",
    )]));
    assert_eq!(found.as_deref(), Some("osc=hspll"));
}

#[test]
fn does_not_misparse_a_string_literal_containing_a_comment_delimiter() {
    let found = find_epic_config(&src(&[(
        "main.c",
        "const char *s = \"/* not a comment */\";\nEPIC_CONFIG(\"osc=hspll\");\n",
    )]));
    assert_eq!(found.as_deref(), Some("osc=hspll"));
}

#[test]
fn finds_it_in_any_of_several_files() {
    let found = find_epic_config(&src(&[
        ("a.c", "void from_a(void) {}\n"),
        ("b.c", "EPIC_CONFIG(\"osc=xt\");\n"),
        ("c.c", "void from_c(void) {}\n"),
    ]));
    assert_eq!(found.as_deref(), Some("osc=xt"));
}

#[test]
#[should_panic(expected = "more than one EPIC_CONFIG")]
fn panics_on_more_than_one_invocation_across_the_whole_program() {
    find_epic_config(&src(&[
        ("a.c", "EPIC_CONFIG(\"osc=xt\");\n"),
        ("b.c", "EPIC_CONFIG(\"osc=hspll\");\n"),
    ]));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `make test CRATE=driver`
Expected: FAIL, `unresolved import driver::prescan`.

- [ ] **Step 3: Implement**

Create `crates/driver/src/prescan.rs`:

```rust
//! A cheap, clang-free scan for `EPIC_CONFIG("...")`'s argument, run before
//! any clang invocation so EPIC_FOSC_HZ can be added to every `-D` list
//! from the start (docs/31 D-10). Comment- and string-literal-aware so a
//! fuse string or a stray comment cannot make it misfire.

/// Scan every source file's raw text for exactly one top-level
/// `EPIC_CONFIG("...")` invocation, skipping `//` and `/* */` comments and
/// `"..."` string literals along the way. Returns the quoted argument, or
/// `None` if no invocation was found anywhere.
///
/// Panics if more than one invocation is found across all files: v1
/// supports exactly one, unconditional, per docs/31 D-10.
pub fn find_epic_config(sources: &[(String, String)]) -> Option<String> {
    let mut found: Option<(String, String)> = None; // (file, spec)
    for (file, text) in sources {
        for spec in find_in_one_file(text) {
            if let Some((prev_file, _)) = &found {
                panic!(
                    "epic-cc: more than one EPIC_CONFIG(...) invocation found \
                     ({prev_file} and {file}); exactly one is supported"
                );
            }
            found = Some((file.clone(), spec));
        }
    }
    found.map(|(_, spec)| spec)
}

fn find_in_one_file(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        // Skip // line comments.
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Skip /* block comments */.
        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        // Skip "string literals", so a comment delimiter or the word
        // EPIC_CONFIG inside one is not mistaken for real source.
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if text[i..].starts_with("EPIC_CONFIG") {
            let after = &text[i + "EPIC_CONFIG".len()..];
            let trimmed = after.trim_start();
            if let Some(rest) = trimmed.strip_prefix('(') {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('"') {
                    if let Some(end) = rest.find('"') {
                        out.push(rest[..end].to_string());
                        i += "EPIC_CONFIG".len();
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}
```

- [ ] **Step 4: Wire the module**

Add `pub mod prescan;` to `crates/driver/src/lib.rs`.

- [ ] **Step 5: Run to verify it passes**

Run: `make test CRATE=driver`
Expected: PASS, all seven new tests.

- [ ] **Step 6: Commit**

```bash
git add crates/driver/src/prescan.rs crates/driver/tests/prescan.rs crates/driver/src/lib.rs
git commit -m "feat(driver): raw-text pre-scan for EPIC_CONFIG, feeds EPIC_FOSC_HZ"
```

---

### Task 6: wire everything into the driver

This is the task that changes `main.rs`'s behavior end to end: writes the header, pre-scans, derives `EPIC_FOSC_HZ`, emits config bytes into the HEX, prints the report.

**Files:**
- Modify: `crates/driver/src/main.rs`

**Interfaces:**
- Consumes: `epic_cc_h::EPIC_CC_H`, `prescan::find_epic_config`, `device::resolve_config`, `asm::to_hex_regions`, `irparse::sanitize_symbols` (already wired by CC-1).
- Produces: the final `epic-cc` binary behavior. Task 7 (gpasm cross-check) and Task 8 (acceptance test) both exercise this.

- [ ] **Step 1: Write the header to a temp include dir, before the clang loop**

In `crates/driver/src/main.rs`, after the `tmp` directory is created (CC-1's existing code) and before the per-unit clang loop:

```rust
    let header_dir = tmp.join("include");
    std::fs::create_dir_all(&header_dir).expect("create header dir");
    std::fs::write(header_dir.join("epic-cc.h"), driver::epic_cc_h::EPIC_CC_H).expect("write epic-cc.h");
```

Add `"-I", header_dir.to_str().unwrap(),` to the clang args list, alongside the existing `-I`/`-D` forwarding, so it is on PATH before any user include but does not require the user to reference it explicitly (matching how `#include <epic-cc.h>` is meant to just work).

- [ ] **Step 2: Pre-scan and resolve `EPIC_FOSC_HZ` before the clang loop**

Immediately before the clang loop (Step 1's header write can happen first or after, order does not matter between them):

```rust
    let sources: Vec<(String, String)> = cli
        .inputs
        .iter()
        .map(|p| (p.clone(), std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("epic-cc: read {p}: {e}");
            std::process::exit(1);
        })))
        .collect();
    let prescan_spec = driver::prescan::find_epic_config(&sources);

    let fosc_hz: u64 = match &prescan_spec {
        Some(spec) => resolve_fosc_hz(device, spec),
        None => resolve_fosc_hz_from_defaults(device),
    };
```

Where `resolve_fosc_hz`/`resolve_fosc_hz_from_defaults` derive a frequency from the resolved `osc`/`plldiv`/`cpudiv`/`usbdiv` (PIC18) or `osc` (PIC14) fields. **This derivation needs a crystal frequency input the fuses alone do not carry** (`osc=hspll` says *how* the clock tree multiplies, not *what* the input crystal is). Add one more field to each device's `ConfigRegion` handling here, not as a `FuseField` (it is not a silicon bit, it has no encoding): read it from a required `xtal_hz=<value>` key in the same `EPIC_CONFIG` string, parsed by `find_epic_config`'s caller, not by `resolve_config`. Extend the pre-scan tests (Task 5) if this parsing needs adjusting, and extend `resolve_config`'s panics to cover `xtal_hz` missing when a PLL/crystal-dependent `osc` value is chosen, since without it `EPIC_FOSC_HZ` cannot be computed at all.

**This sub-step is underspecified on purpose.** The frequency arithmetic for PIC18F4550's `PLLDIV`/`CPUDIV`/`USBDIV` chain and PIC16F877A's four `osc` modes needs cross-checking against DS39632E §2.2 and DS39582C §14.2 before being written as fact, the same discipline Tasks 2/3 applied to the config-bit tables themselves. Read those sections from the vendored PDFs, write `resolve_fosc_hz`/`resolve_fosc_hz_from_defaults` with the derived arithmetic, and add unit tests asserting specific `(osc, plldiv, cpudiv, usbdiv, xtal_hz) -> Hz` combinations before wiring them into `main.rs`. Do not guess the PLL/postscaler arithmetic from memory.

- [ ] **Step 3: Add `-D EPIC_FOSC_HZ` to every clang invocation**

In the existing per-unit clang `Command` construction (CC-1's loop), add:

```rust
        cmd.args(["-D", &format!("EPIC_FOSC_HZ={fosc_hz}")]);
```

- [ ] **Step 4: After the merge, extract the authoritative `EPIC_CONFIG` payload and cross-check it**

After `sanitize_symbols` and before `parse_ll` (or right after `parse_ll`, either works since the section-tagged dummy global survives sanitization unchanged, being an ordinary global), search the merged `.ll` text for the `.epiccfg.` section prefix the same way `irparse`'s `EPIC_AT` handling does in Task 1, extracting everything between `.epiccfg.` and the closing quote as the authoritative spec string:

```rust
    let canonical_spec = ll_text
        .find("section \".epiccfg.")
        .map(|i| &ll_text[i + "section \".epiccfg.".len()..])
        .and_then(|rest| rest.split('"').next())
        .map(str::to_string);

    match (&prescan_spec, &canonical_spec) {
        (Some(p), Some(c)) if p != c => panic!(
            "epic-cc: internal inconsistency, the pre-scan found EPIC_CONFIG({p:?}) but the \
             compiled program's actual config is {c:?}; this is a pre-scanner bug, please report it"
        ),
        (Some(_), None) => panic!(
            "epic-cc: the pre-scan found an EPIC_CONFIG(...) invocation that did not survive \
             into the compiled program (likely behind an #ifdef the pre-scan cannot see); v1 \
             requires an unconditional top-level invocation"
        ),
        _ => {}
    }
```

- [ ] **Step 5: Resolve and emit config bytes**

Using `canonical_spec.as_deref().unwrap_or("")` (an empty spec is valid: it means every field, except the required oscillator ones, which will panic via `resolve_config` if genuinely unmentioned) against `device.config`, call `resolve_config`, then emit:

```rust
    let config_bytes: Vec<u8> = device::resolve_config(&device.config, canonical_spec.as_deref().unwrap_or(""));
```

For **PIC14**, fold this into the existing single `words: Vec<u16>` the program's own `to_hex` call already uses: extend that vector to at least `device.config.base_byte_addr / 2 + 1` entries (zero-padded), set the one config word at that index from `config_bytes` (`u16::from(config_bytes[0]) | (u16::from(config_bytes[1]) << 8)`), and call the existing `asm::assemble_file_to_hex` path, first widening its `words.len() <= device.flash_words` assert (`crates/asm/src/lib.rs:417`) to compare against a new `device`-level "total addressable word space" rather than `flash_words`, since a word at `0x2007` is legitimately past an 8K-word (`0x2000`) program's own flash ceiling.

For **PIC18**, this is where `to_hex_regions` (Task 4) replaces the call to `asm::assemble_file_to_hex`'s internal `to_hex`: pack `config_bytes` two-per-word (even offset = low byte, odd offset = high byte, matching the `DB` convention `crates/asm/src/lib.rs:116-118` already documents), and call `to_hex_regions(&[(0, &program_words), (device.config.base_byte_addr, &config_words)])` instead of the single-region path.

**This step touches `asm::assemble_file_to_hex`'s PIC14 assert and PIC18's call site, both load-bearing for every existing fixture.** Run `make test` (full suite) immediately after this step, before moving on, and confirm `git diff --stat crates/driver/tests/fixtures/` (the `.c` files; `.hex` is gitignored, same caveat as CC-1) shows nothing changed.

- [ ] **Step 6: Print the resolved-config report unconditionally**

After emitting the HEX, before returning:

```rust
    eprintln!("epic-cc: resolved configuration for {}:", device.name);
    for (i, b) in config_bytes.iter().enumerate() {
        eprintln!("  byte 0x{:06X} = 0x{b:02X}", device.config.base_byte_addr as usize + i);
    }
```

D-4's promise is that "safe defaults never means mystery silicon state"; naming which *fields* resolved to which named value (not just the raw bytes) is a further improvement worth doing here if time allows, but the raw byte report alone already satisfies the promise as stated.

- [ ] **Step 7: fix `crates/sim::parse_hex`'s fixed-size buffer, a confirmed bug this task exposes**

Checked directly against the source (`crates/sim/src/lib.rs:6`): `parse_hex` allocates `vec![0u16; 8192]` and writes `words[addr / 2 + i] = w` with no bounds check. A PIC16F877A config word at word index `0x2007` (8199) is past that buffer and panics with an out-of-bounds index, not a clean error. Task 8's acceptance test needs to round-trip exactly that word through the simulator, so this is a real blocker, not a hypothetical.

Fix `crates/sim/src/lib.rs`'s `parse_hex` to size the buffer from the data actually present, in a first pass, before writing:

```rust
pub fn parse_hex(data: &str) -> Vec<u16> {
    let mut max_word = 8191usize; // never shrink below the historical minimum
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let bytes = hex_decode(&line[1..]);
        let len = bytes[0] as usize;
        let addr = ((bytes[1] as usize) << 8) | (bytes[2] as usize);
        if bytes[3] == 0x00 {
            max_word = max_word.max(addr / 2 + len / 2);
        }
    }
    let mut words = vec![0u16; max_word + 1];
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        assert!(line.starts_with(':'), "not Intel HEX: {line}");
        let bytes = hex_decode(&line[1..]);
        let len = bytes[0] as usize;
        let addr = ((bytes[1] as usize) << 8) | (bytes[2] as usize);
        let rectype = bytes[3];
        let data = &bytes[4..4 + len];
        match rectype {
            0x00 => {
                for (i, chunk) in data.chunks(2).enumerate() {
                    let w = (chunk[0] as u16) | ((chunk[1] as u16) << 8);
                    words[addr / 2 + i] = w;
                }
            }
            0x01 => break,
            0x04 => {}
            other => panic!("unsupported HEX record type {other:#x}"),
        }
    }
    words
}
```

Run `make test CRATE=sim` after this change and confirm every pre-existing `sim` test still passes; this only widens the buffer, it does not change any address's resolved value for HEX files that already fit in 8192 words.

**`parse_hex_pic18` has a separate, more serious bug this plan does not fix.** It silently ignores every `0x04` extended-linear-address record (`0x04 => {}`, the window value is read and discarded), so a chunk written at a non-zero window (exactly what `to_hex_regions` produces for PIC18's config bytes at `0x300000`) gets aliased onto low addresses in the fixed `0x4000`-word buffer instead of erroring or landing at the right place. Task 8's acceptance test only exercises the PIC14 path, so this plan does not require fixing it, but it is a real, now-confirmed gap: anyone later writing a PIC18-targeted config-word simulator test will hit silent corruption, not a clean failure, until `parse_hex_pic18` is taught to track the current window the same way `parse_hex`'s bugfix above tracks buffer size. Flag this in the final ADR distillation (see "Done when") so it is not rediscovered from scratch.

- [ ] **Step 8: Run the full suite**

Run: `make test`
Expected: PASS, all 16 crates (the same isolated-`CARGO_TARGET_DIR` caution from CC-1's session applies if this host still has other worktrees actively building; see that session's notes if `isel-pic18` fails with test names that do not exist in this branch's source).

- [ ] **Step 9: Commit**

```bash
git add crates/driver/src/main.rs crates/asm/src/lib.rs crates/sim/src/lib.rs
git commit -m "feat(driver): emit config words and EPIC_FOSC_HZ end to end"
```

---

### Task 7: `gpasm` cross-check tests

**Files:**
- Create: `crates/driver/tests/fixtures/gpasm_config_pic14.asm`
- Create: `crates/driver/tests/fixtures/gpasm_config_pic18.asm`
- Create: `crates/driver/tests/gpasm_config.rs`

**Interfaces:**
- Consumes: `device::resolve_config`, the two real `ConfigRegion`s.
- Produces: nothing other tasks depend on; this is a verification-only task.

- [ ] **Step 1: Write the gpasm fixtures**

Create `crates/driver/tests/fixtures/gpasm_config_pic14.asm`:

```asm
	list p=16f877a
	radix hex
#include <p16f877a.inc>
	__CONFIG _CP_OFF & _WDT_OFF & _BODEN_ON & _PWRTE_ON & _LVP_OFF & _CPD_OFF & _WRT_OFF & _DEBUG_OFF & _XT_OSC
	org 0
	nop
	end
```

Create `crates/driver/tests/fixtures/gpasm_config_pic18.asm`:

```asm
	list p=18f4550
	radix hex
#include <p18f4550.inc>
	__CONFIG _CONFIG4L, _DEBUG_OFF_4L & _XINST_OFF_4L & _ICPRT_OFF_4L & _LVP_OFF_4L & _STVREN_ON_4L
	org 0
	nop
	end
```

Both were assembled by hand during this plan's own research (2026-08-21) and their output bytes (`0x3F71` and `0x9B` respectively) are already asserted directly in Task 2 and Task 3's unit tests. This task automates re-running that same cross-check as part of the test suite, rather than trusting a one-time manual result forever.

- [ ] **Step 2: Write the test that re-runs gpasm and diffs**

Create `crates/driver/tests/gpasm_config.rs`:

```rust
use std::process::Command;

fn gpasm_hex(asm_path: &str) -> String {
    let out_path = format!("{asm_path}.hex");
    let status = Command::new("gpasm")
        .args(["-o", &out_path, asm_path])
        .status()
        .expect("run gpasm");
    assert!(status.success(), "gpasm failed on {asm_path}");
    std::fs::read_to_string(&out_path).expect("read gpasm hex")
}

/// Pull the data bytes out of the one :02 record at `want_addr` (an Intel
/// HEX line like `:02400E00713F00`: count, address, type, data..., sum).
fn bytes_at(hex: &str, want_addr: u32) -> Vec<u8> {
    for line in hex.lines() {
        let rec = line.trim_start_matches(':');
        if rec.len() < 8 {
            continue;
        }
        let count = u8::from_str_radix(&rec[0..2], 16).unwrap();
        let addr = u16::from_str_radix(&rec[2..6], 16).unwrap();
        let rtype = &rec[6..8];
        if rtype != "00" || addr as u32 != want_addr {
            continue;
        }
        let data = &rec[8..8 + 2 * count as usize];
        return (0..count as usize)
            .map(|i| u8::from_str_radix(&data[2 * i..2 * i + 2], 16).unwrap())
            .collect();
    }
    panic!("no data record at address 0x{want_addr:04X} in:\n{hex}");
}

#[test]
fn pic16f877a_matches_gpasm() {
    let hex = gpasm_hex("tests/fixtures/gpasm_config_pic14.asm");
    let bytes = bytes_at(&hex, 0x000E); // low 16 bits of byte address 0x400E
    let ours = device::resolve_config(
        &device::PIC16F877A.config,
        "osc=xt, wdt=off, pwrt=on, bor=on, lvp=off, cpd=off, wrt=off, debug=off, cp=off",
    );
    assert_eq!(bytes, ours);
}

#[test]
fn pic18f4550_config4l_matches_gpasm() {
    let hex = gpasm_hex("tests/fixtures/gpasm_config_pic18.asm");
    let bytes = bytes_at(&hex, 0x0006); // low 16 bits of byte address 0x300006
    let ours = device::resolve_config(
        &device::PIC18F4550.config,
        "osc=hspll, plldiv=div5, cpudiv=div1, usbdiv=on, \
         debug=off, xinst=off, icprt=off, lvp=off, stvren=on",
    );
    assert_eq!(&bytes[..1], &ours[6..7]);
}
```

**`bytes_at`'s address matching only checks the low 16 bits.** For the PIC18 fixture that is deliberate (the file has no other content, so `gpasm`'s own extended-address record puts everything at `upper=0x0030`, and the test only needs the offset within that window). If a future fixture adds real program content before the config directive, revisit this to also check the extended-address record's upper bits, not just the low 16.

- [ ] **Step 3: Run**

Run: `make test CRATE=driver`
Expected: PASS, both tests. If either fails, the failure names a specific config table entry to re-check against the datasheet, not a test to relax.

- [ ] **Step 4: Commit**

```bash
git add crates/driver/tests/fixtures/gpasm_config_pic14.asm crates/driver/tests/fixtures/gpasm_config_pic18.asm crates/driver/tests/gpasm_config.rs
git commit -m "test(driver): gpasm cross-check for both devices' config words"
```

---

### Task 8: end-to-end acceptance test

**Files:**
- Create: `crates/driver/tests/fixtures/config_probe.c`
- Create: `crates/driver/tests/config_e2e.rs`

**Interfaces:**
- Consumes: the fully wired driver from Task 6.
- Produces: nothing other tasks depend on.

- [ ] **Step 1: Write the fixture**

Create `crates/driver/tests/fixtures/config_probe.c`, exercising `EPIC_AT` and `EPIC_CONFIG` together:

```c
// CC-3 acceptance: EPIC_AT places `out` at a fixed address; EPIC_CONFIG
// sets a real, checkable fuse combination; EPIC_FOSC_HZ is read back into
// a global so the test can assert the driver derived it correctly.
#include <epic-cc.h>

EPIC_CONFIG("osc=xt, xtal_hz=4000000, wdt=off, lvp=off");

volatile unsigned char out EPIC_AT(0x0021);
unsigned long fosc = EPIC_FOSC_HZ;

void main(void) {
    out = 0x2A;
}
```

**`xtal_hz` here assumes Task 6's Step 2 sub-step landed as specified.** If that sub-step's design changed during implementation (the PLL/postscaler arithmetic genuinely needed different inputs once DS39632E §2.2 was read), update this fixture to match whatever `EPIC_CONFIG` syntax Task 6 actually shipped, and update the assertion in Step 2 below accordingly; do not leave this fixture asserting a syntax the driver does not accept.

- [ ] **Step 2: Write the test**

Create `crates/driver/tests/config_e2e.rs`:

```rust
use std::process::Command;

#[test]
fn places_a_global_and_resolves_config_end_to_end() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/config_probe.c",
            "-o",
            "tests/fixtures/config_probe.hex",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    // The resolved-config report is on stderr, unconditional per D-4.
    let report = String::from_utf8_lossy(&out.stderr);
    assert!(report.contains("resolved configuration for p16f877a"), "{report}");

    let hex = std::fs::read_to_string("tests/fixtures/config_probe.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(1000);
    assert!(p.halted());
    // `out` was pinned to 0x0021 by EPIC_AT; irparse's placement reading
    // (Task 1) is what makes this address, not alloc's own choice, land.
    assert_eq!(p.ram()[0x0021], 0x2A);
}
```

**This test depends on Task 6 Step 7's `parse_hex` fix having landed.** The config word at `0x2007` sits beyond the 877A's `0x2000`-word program space; `parse_hex`'s original fixed `vec![0u16; 8192]` buffer would panic out of bounds trying to place it, confirmed directly against the source during this plan's research, not assumed. If Task 6 Step 7 was skipped, fix it before this test, not after: a panic here means that step is missing, not that this fixture is wrong.

- [ ] **Step 3: Run**

Run: `make test CRATE=driver`
Expected: PASS.

- [ ] **Step 4: Run the full suite one more time**

Run: `make test`
Expected: PASS, all 16 crates.

- [ ] **Step 5: Commit**

```bash
git add crates/driver/tests/fixtures/config_probe.c crates/driver/tests/config_e2e.rs
git commit -m "test(driver): EPIC_AT + EPIC_CONFIG + EPIC_FOSC_HZ end to end"
```

---

## Done when

- `make test` passes, full suite, 16/16 crates.
- `EPIC_AT(addr)` places a global at a literal address, provable by reading it back at that address in `crates/sim`.
- `EPIC_CONFIG("...")` resolves to the correct bytes for both devices, cross-checked against `gpasm`, not just hand-verified.
- An `EPIC_CONFIG` override of `xinst=on` panics naming the field.
- Omitting a required (oscillator-tree) field panics naming it and its valid values.
- `EPIC_FOSC_HZ` is usable in a genuine preprocessor context (a `#if` or a compile-time array bound would both work; the acceptance fixture only proves the simpler "read into a global" case, which is enough to prove the mechanism, not the full breadth of what a macro conventionally supports).
- The resolved-config report prints unconditionally on success.
- Every pre-existing PIC14 fixture's `.hex` output is unaffected (`to_hex` untouched; only the driver's own call sites changed to route through the widened assert and, for PIC18, `to_hex_regions`).
- `make pre-pr-check` clean, plan file `git rm`ed, and this plan's real findings folded back into `docs/31-ecosystem-integration-design.md` D-9/D-10 or a new ADR, matching CC-1's precedent (ADR-011). At minimum: the `EPIC_FOSC_HZ` derivation arithmetic actually used (once Task 6 Step 2's underspecified sub-step is resolved with real datasheet sections), the `parse_hex` buffer-sizing fix, and `parse_hex_pic18`'s unfixed extended-address-record gap (below), so it is discoverable by name rather than rediscovered from a confusing test failure.

## Known gaps, deliberately left open

**Task 6 Step 2's `EPIC_FOSC_HZ` frequency arithmetic is not written out as concrete code in this plan.** Every other piece of data in this plan (every config-bit position, every mask, both erased baselines) was read from the actual datasheet PDFs and cross-checked against `gpasm` during this plan's own research session. The PLL/postscaler-to-Hz arithmetic (DS39632E §2.2's clock diagram, DS39582C §14.2's four oscillator modes) was not extracted with the same rigor before this plan was written, and writing plausible-looking divider arithmetic from general PIC knowledge, the exact failure mode this whole plan exists to avoid for the config bits, would be worse than leaving it explicitly flagged. Whoever executes Task 6 must read those two sections from the vendored PDFs first.

**`parse_hex_pic18` silently ignores the `0x04` extended-linear-address record.** Confirmed directly against `crates/sim/src/lib.rs:447-471` during this plan's research: the `0x04 => {}` match arm reads and discards the window value, so a chunk written at a non-zero window (`to_hex_regions`'s PIC18 config-byte output, at `0x300000`) would silently alias onto low program addresses in the simulator rather than landing at the right place or erroring. Task 8's acceptance test only exercises the PIC14 path, so this plan does not require fixing it, but it is real and now confirmed, not speculative. Whoever next writes a PIC18-targeted config-word simulator test should expect to fix this first, using the same first-pass buffer-sizing approach Task 6 Step 7 applies to `parse_hex`, extended to also track the current window from `0x04` records and offset each `0x00` record's address by it.
