# Device Provenance and gputils Cross-Check Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** make a wrong number in `crates/device/devices/*.toml` fail a build instead of silently miscompiling.

**Architecture:** every device TOML gains a required `[provenance]` table that `build.rs` refuses to build without. A new Rust integration test cross-checks each device against gputils, which is already in the dev image: `flash_words` by assembling two `org` probes with `gpasm`, and the RAM map by parsing the generic `.lkr` and comparing coalesced address ranges. Nothing from gputils is copied into the tree.

**Tech Stack:** Rust 1.97.1, `toml` 0.8 (already a build-dependency), `gpasm` 1.5.2 and gputils data under `/usr/local/share/gputils`, all inside the `epic-cc-dev` docker image.

**Spec:** `docs/superpowers/specs/2026-08-23-device-provenance-design.md`

## Global Constraints

- Everything runs in docker. `make test`, `make test CRATE=device`, `make exec CMD='...'`. Never install a toolchain on the host. Avoid double quotes inside `CMD`.
- Conventional Commits, single line, no trailers (no `Co-Authored-By`), no em-dash characters anywhere.
- Work happens in the worktree `.worktrees/feat-104-provenance` on branch `feat/104-device-provenance`. Never on master.
- Comments carry the non-obvious reason, not a restatement of the code, and stay at 3 lines or fewer.
- `cargo build --workspace --all-targets` must stay warning-free (`make check-warnings`).
- This plan deletes itself in the final commit (Task 7); decisions distill into an ADR.

## Measured facts this plan depends on

Verified in the dev image before writing. Do not re-derive; do not assume differently.

- `.lkr` path is `<PIC8_GPUTILS_SHARE>/lkr/<stem-without-leading-p>_g.lkr`, e.g. `p16f877a` -> `16f877a_g.lkr`.
- gputils GPR lines look like `DATABANK   NAME=gpr0       START=0x20              END=0x6F` and `SHAREBANK  NAME=gprnobnk   START=0x70            END=0x7F`. Lines ending in `PROTECTED` must be excluded (they are SFR banks and the common-RAM mirrors at `0xF0`, `0x170`, `0x1F0`).
- Only `DATABANK` entries whose `NAME` begins `gpr` count. `sfr0..sfr3` are excluded by that rule and by `PROTECTED`.
- **Comparison must be union-and-coalesce, never element-wise.** For `p18f4550` gputils lists nine ranges (`gpre` `0x0-0x5F` plus `gpr0..gpr7` `0x60-0x7FF`) where our TOML has one `ram_banks` entry `[0x10,0x7FF]` plus `common_ram` `[0x0,0xF]`. Both coalesce to `0x0-0x7FF`. An element-wise check fails on correct data.
- With coalescing, all three shipped devices currently MATCH. There is no remediation work hiding in this plan.
- Negative controls confirmed to produce a mismatch: the `#101` regression (`0x120`/`0x1A0` bank starts) and the `#88` falsification (banks out to `0x3FF`).
- `gpasm` flash probe, marker string `Address exceeds maximum range` inside `Warning[220]`. gpasm exits 0 on warnings, so match on stderr, never on exit status.
- **`org` is word-addressed on PIC14 and byte-addressed on PIC18.** Measured: `p16f877a` clean at `org 0x1FFF` and warns at `org 0x2000`; `p18f4550` errors `ORG at odd address` at `org 0x3FFF`, is clean at `org 0x7FFE`, and warns at `org 0x8000`.
- The image's `python3` is 3.10.12, so it has no `tomllib`. Task 6 must not rely on it.

## File structure

| File | Responsibility |
|---|---|
| `crates/device/provenance.rs` (create) | Pure validation of the `[provenance]` table. `include!`d by both `build.rs` and its test, so the rule has exactly one definition. |
| `crates/device/build.rs` (modify) | Calls the validator while looping devices. |
| `crates/device/devices/*.toml` (modify) | Gain the `[provenance]` stanza. |
| `crates/device/tests/provenance.rs` (create) | Unit-tests the rule, and asserts every shipped TOML satisfies it. |
| `crates/device/src/gputils.rs` (create) | Pure `.lkr` parsing and range coalescing. No I/O beyond reading a string. |
| `crates/device/tests/gputils_crosscheck.rs` (create) | The gate: iterates `device::ALL`, compares RAM, runs the flash probes. |
| `crates/device/Cargo.toml` (modify) | Adds `toml` as a dev-dependency for the provenance test. |
| `scripts/gen-device.py` (modify) | Emits the provenance stanza it now must produce. |
| `.github/workflows/ci.yml` (modify) | Runs the ATDF check inside the image and makes a skip unmistakable. |

---

### Task 1: The provenance rule, as a shared pure function

**Files:**
- Create: `crates/device/provenance.rs`
- Create: `crates/device/tests/provenance.rs`
- Modify: `crates/device/Cargo.toml`

**Interfaces:**
- Consumes: nothing.
- Produces: `fn validate_provenance(path: &str, root: &toml::Value) -> Result<(), String>`. Task 2 calls it from `build.rs`.

- [ ] **Step 1: Add `toml` as a dev-dependency**

In `crates/device/Cargo.toml`, under the existing `[dev-dependencies]` block, add:

```toml
toml = "0.8"
```

- [ ] **Step 2: Write the failing test**

Create `crates/device/tests/provenance.rs`:

```rust
//! The `[provenance]` rule, tested directly and against every shipped TOML.
//! `build.rs` includes the same file, so there is one definition of the rule.

include!("../provenance.rs");

fn parse(s: &str) -> toml::Value {
    s.parse::<toml::Value>().expect("test TOML must parse")
}

const ATDF: &str = r#"
[provenance]
tier = "atdf"
source = "PIC16F877A.atdf"
pack = "Microchip.PIC16Fxxx_DFP.1.7.162"
sha256 = "abc123"
"#;

#[test]
fn accepts_a_complete_atdf_stanza() {
    assert_eq!(validate_provenance("t.toml", &parse(ATDF)), Ok(()));
}

#[test]
fn rejects_a_missing_stanza() {
    let e = validate_provenance("t.toml", &parse("name = \"x\"\n")).unwrap_err();
    assert!(e.contains("missing [provenance]"), "{e}");
}

#[test]
fn rejects_an_unknown_tier() {
    let src = "[provenance]\ntier = \"vibes\"\n";
    let e = validate_provenance("t.toml", &parse(src)).unwrap_err();
    assert!(e.contains("tier"), "{e}");
}

#[test]
fn rejects_atdf_tier_without_sha256() {
    let src = "[provenance]\ntier = \"atdf\"\nsource = \"a.atdf\"\npack = \"p\"\n";
    let e = validate_provenance("t.toml", &parse(src)).unwrap_err();
    assert!(e.contains("sha256"), "{e}");
}

#[test]
fn rejects_datasheet_tier_without_a_ticket() {
    let src = "[provenance]\ntier = \"datasheet\"\ndocument = \"DS39582C\"\n";
    let e = validate_provenance("t.toml", &parse(src)).unwrap_err();
    assert!(e.contains("ticket"), "{e}");
}

#[test]
fn rejects_datasheet_tier_without_a_document() {
    let src = "[provenance]\ntier = \"datasheet\"\nticket = \"epic-cc#92\"\n";
    let e = validate_provenance("t.toml", &parse(src)).unwrap_err();
    assert!(e.contains("document"), "{e}");
}

#[test]
fn every_shipped_device_has_valid_provenance() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/devices");
    let mut seen = 0;
    for ent in std::fs::read_dir(dir).expect("devices dir") {
        let path = ent.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let root = text.parse::<toml::Value>().unwrap();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(validate_provenance(&name, &root), Ok(()), "{name}");
        seen += 1;
    }
    assert!(seen > 0, "no device TOMLs found");
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `make exec CMD='cargo test -p device --test provenance'`
Expected: FAIL to compile, `provenance.rs` does not exist.

- [ ] **Step 4: Write the rule**

Create `crates/device/provenance.rs`:

```rust
// Device numbers are the one input with no oracle upstream of banking and
// paging, so a TOML must say where its values came from. Included by both
// build.rs and tests/provenance.rs to keep a single definition.

fn validate_provenance(path: &str, root: &toml::Value) -> Result<(), String> {
    let p = root
        .get("provenance")
        .ok_or_else(|| format!("device: {path}: missing [provenance] table"))?;
    let field = |k: &str| p.get(k).and_then(|v| v.as_str());
    let tier = field("tier")
        .ok_or_else(|| format!("device: {path}: [provenance] needs a tier"))?;

    let required: &[&str] = match tier {
        "atdf" => &["source", "pack", "sha256"],
        "datasheet" => &["document", "ticket"],
        other => {
            return Err(format!(
                "device: {path}: unknown provenance tier {other:?}, expected atdf or datasheet"
            ))
        }
    };
    for key in required {
        if field(key).is_none_or(str::is_empty) {
            return Err(format!(
                "device: {path}: provenance tier {tier:?} requires a non-empty {key}"
            ));
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Run the test to verify the rule tests pass**

Run: `make exec CMD='cargo test -p device --test provenance'`
Expected: the six rule tests PASS; `every_shipped_device_has_valid_provenance` FAILS, because no TOML has the stanza yet. That is the correct intermediate state and Task 2 fixes it.

- [ ] **Step 6: Commit**

```bash
git add crates/device/provenance.rs crates/device/tests/provenance.rs crates/device/Cargo.toml
git commit -m "test(device): rule for the required provenance stanza"
```

---

### Task 2: Enforce provenance in build.rs and backfill the TOMLs

**Files:**
- Modify: `crates/device/build.rs`
- Modify: `crates/device/devices/p16f877a.toml`, `p16f887.toml`, `p18f4550.toml`

**Interfaces:**
- Consumes: `validate_provenance` from Task 1.
- Produces: a build that refuses an unattested device TOML.

- [ ] **Step 1: Include and call the validator in build.rs**

At the top of `crates/device/build.rs`, after the existing `use` lines, add:

```rust
include!("provenance.rs");
```

`build.rs` already parses each file into a typed `DeviceToml`. Add a second parse into a generic `toml::Value` so the validator can inspect the table. In the per-file loop in `main()`, immediately after the existing `let content = fs::read_to_string(&path)...` line and before the typed parse, insert:

```rust
        let raw: toml::Value = content
            .parse()
            .unwrap_or_else(|e| panic!("device: parse {}: {e}", path.display()));
        if let Err(msg) = validate_provenance(path.file_name().unwrap().to_str().unwrap(), &raw) {
            panic!("{msg}");
        }
```

- [ ] **Step 2: Run the build to verify it now fails**

Run: `make exec CMD='cargo build -p device'`
Expected: FAIL with `device: p16f877a.toml: missing [provenance] table`.

- [ ] **Step 3: Backfill the two PIC14 TOMLs**

`#92` recorded the datasheets, so both start at the datasheet tier. Append to `crates/device/devices/p16f877a.toml`:

```toml

[provenance]
tier = "datasheet"
document = "DS39582C"
tables = ["TABLE 2-1: Register File Map"]
ticket = "epic-cc#92"
```

Append to `crates/device/devices/p16f887.toml`:

```toml

[provenance]
tier = "datasheet"
document = "DS41291D"
tables = ["TABLE 2-1: Register File Map"]
ticket = "epic-cc#92"
```

- [ ] **Step 4: Backfill p18f4550, looking up its datasheet number**

No document is recorded for this part anywhere in the repo, so find it rather than inherit one. Run:

```bash
make exec CMD='grep -riE "DS[0-9]{5}" /usr/local/share/gputils/header/p18f4550.inc | head -3'
```

If that yields nothing, take the document number from the datasheet PDF referenced in `docs/06-environment.md`. Then append to `crates/device/devices/p18f4550.toml`:

```toml

[provenance]
tier = "datasheet"
document = "<the document number you found>"
tables = ["Memory Organization"]
ticket = "epic-cc#104"
```

Do not invent a number. If you cannot find one, stop and report it.

- [ ] **Step 5: Run the tests to verify everything passes**

Run: `make exec CMD='cargo test -p device'`
Expected: PASS, including `every_shipped_device_has_valid_provenance`.

- [ ] **Step 6: Commit**

```bash
git add crates/device/build.rs crates/device/devices
git commit -m "feat(device): require a provenance stanza on every device toml"
```

---

### Task 3: Parse the gputils linker script and coalesce ranges

**Files:**
- Create: `crates/device/src/gputils.rs`
- Modify: `crates/device/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub fn coalesce(ranges: &[(u16, u16)]) -> Vec<(u16, u16)>`
  - `pub fn gpr_ranges_from_lkr(text: &str) -> Vec<(u16, u16)>`

  Both are pure. Task 4 calls them.

- [ ] **Step 1: Write the failing test**

Create `crates/device/src/gputils.rs` with the tests only for now:

```rust
//! Parsing of the gputils generic linker script, used only to cross-check the
//! committed device data. Nothing here is copied into a device TOML.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesce_merges_adjacent_and_overlapping_ranges() {
        assert_eq!(coalesce(&[(0x20, 0x6F), (0x70, 0x7F)]), vec![(0x20, 0x7F)]);
        assert_eq!(coalesce(&[(0x20, 0x40), (0x30, 0x7F)]), vec![(0x20, 0x7F)]);
        assert_eq!(
            coalesce(&[(0xA0, 0xEF), (0x20, 0x6F)]),
            vec![(0x20, 0x6F), (0xA0, 0xEF)]
        );
    }

    const LKR: &str = "\
DATABANK   NAME=sfr0       START=0x0               END=0x1F           PROTECTED
DATABANK   NAME=gpr0       START=0x20              END=0x6F
DATABANK   NAME=gpr1       START=0xA0              END=0xEF
SHAREBANK  NAME=gprnobnk   START=0x70            END=0x7F
SHAREBANK  NAME=gprnobnk   START=0xF0            END=0xFF           PROTECTED
CODEPAGE   NAME=page0      START=0x0               END=0x7FF
";

    #[test]
    fn parses_gpr_databanks_and_unprotected_sharebanks() {
        assert_eq!(
            gpr_ranges_from_lkr(LKR),
            vec![(0x20, 0x6F), (0x70, 0x7F), (0xA0, 0xEF)]
        );
    }

    #[test]
    fn skips_protected_lines_and_non_gpr_databanks() {
        let got = gpr_ranges_from_lkr(LKR);
        assert!(!got.contains(&(0x0, 0x1F)), "sfr0 must not count");
        assert!(!got.contains(&(0xF0, 0xFF)), "protected mirror must not count");
    }
}
```

Register the module in `crates/device/src/lib.rs` by adding near the other module declarations:

```rust
pub mod gputils;
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `make exec CMD='cargo test -p device gputils'`
Expected: FAIL to compile, `coalesce` and `gpr_ranges_from_lkr` not found.

- [ ] **Step 3: Implement the two functions**

Add above the `mod tests` block in `crates/device/src/gputils.rs`:

```rust
/// Merges overlapping and adjacent ranges into maximal disjoint ones.
/// Adjacency matters: gputils splits a flat PIC18 window into nine banks that
/// describe the same memory as one TOML entry.
pub fn coalesce(ranges: &[(u16, u16)]) -> Vec<(u16, u16)> {
    let mut sorted = ranges.to_vec();
    sorted.sort_unstable();
    let mut out: Vec<(u16, u16)> = Vec::new();
    for (lo, hi) in sorted {
        match out.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}

/// General-purpose RAM described by a generic `.lkr`: `DATABANK` entries named
/// `gpr*` plus unprotected `SHAREBANK`s. `PROTECTED` marks SFR banks and the
/// common-RAM mirrors, which are not allocatable.
pub fn gpr_ranges_from_lkr(text: &str) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let kind = if let Some(r) = line.strip_prefix("DATABANK") {
            ("DATABANK", r)
        } else if let Some(r) = line.strip_prefix("SHAREBANK") {
            ("SHAREBANK", r)
        } else {
            continue;
        };
        if kind.1.contains("PROTECTED") {
            continue;
        }
        let field = |key: &str| -> Option<&str> {
            kind.1
                .split_whitespace()
                .find_map(|t| t.strip_prefix(key))
        };
        let name = field("NAME=").unwrap_or("");
        if kind.0 == "DATABANK" && !name.starts_with("gpr") {
            continue;
        }
        let hex = |s: &str| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok();
        if let (Some(lo), Some(hi)) = (
            field("START=").and_then(hex),
            field("END=").and_then(hex),
        ) {
            out.push((lo, hi));
        }
    }
    out.sort_unstable();
    out
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `make exec CMD='cargo test -p device gputils'`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/device/src/gputils.rs crates/device/src/lib.rs
git commit -m "feat(device): parse gpr ranges from the gputils linker script"
```

---

### Task 4: The RAM cross-check across the whole registry

**Files:**
- Create: `crates/device/tests/gputils_crosscheck.rs`

**Interfaces:**
- Consumes: `device::gputils::{coalesce, gpr_ranges_from_lkr}`, `device::ALL`.
- Produces: `fn gputils_share() -> Option<std::path::PathBuf>`, reused by Task 5 in the same file.

- [ ] **Step 1: Write the failing test**

Create `crates/device/tests/gputils_crosscheck.rs`:

```rust
//! The device data gate. gputils ships in the dev image, so unlike the ATDF
//! check this cannot be skipped for want of a download.

use device::gputils::{coalesce, gpr_ranges_from_lkr};
use std::path::PathBuf;

/// gputils data root. A missing tool fails rather than skips: a gate that
/// disappears with its tool is not a gate.
fn gputils_share() -> Option<PathBuf> {
    if std::env::var("PIC8_ALLOW_NO_GPUTILS").is_ok() {
        return None;
    }
    let dir = std::env::var("PIC8_GPUTILS_SHARE")
        .unwrap_or_else(|_| "/usr/local/share/gputils".into());
    let path = PathBuf::from(dir);
    assert!(
        path.is_dir(),
        "gputils data not found at {}. Set PIC8_GPUTILS_SHARE, or \
         PIC8_ALLOW_NO_GPUTILS=1 to knowingly run without the gate.",
        path.display()
    );
    Some(path)
}

/// `p16f877a` -> `16f877a_g.lkr`.
fn lkr_for(share: &PathBuf, name: &str) -> Option<String> {
    let stem = name.strip_prefix('p').unwrap_or(name);
    let path = share.join("lkr").join(format!("{stem}_g.lkr"));
    std::fs::read_to_string(path).ok()
}

#[test]
fn ram_map_matches_gputils_for_every_device() {
    let Some(share) = gputils_share() else { return };
    let mut checked = 0;
    for dev in device::ALL {
        let Some(lkr) = lkr_for(&share, dev.name) else {
            // No .lkr means this part is not covered; provenance must then be
            // the datasheet tier, which Task 1's rule already enforces.
            continue;
        };
        let mut ours: Vec<(u16, u16)> = dev.ram_banks.to_vec();
        if let Some(cr) = dev.common_ram {
            ours.push(cr);
        }
        let ours = coalesce(&ours);
        let theirs = coalesce(&gpr_ranges_from_lkr(&lkr));
        let fmt = |rs: &[(u16, u16)]| {
            rs.iter()
                .map(|(a, b)| format!("{a:#06X}-{b:#06X}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(
            ours,
            theirs,
            "{}: RAM map disagrees with gputils\n  ours   : {}\n  gputils: {}",
            dev.name,
            fmt(&ours),
            fmt(&theirs)
        );
        checked += 1;
    }
    assert!(checked > 0, "no device was cross-checked");
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `make exec CMD='cargo test -p device --test gputils_crosscheck'`
Expected: PASS. All three shipped devices already agree with gputils under coalescing; this was measured before the plan was written.

- [ ] **Step 3: Prove the gate bites, using the `#101` regression**

Temporarily edit `crates/device/devices/p16f877a.toml`, changing `ram_banks` bank 2 and 3 starts back to the pre-`#101` values:

```toml
ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF], [0x0120, 0x016F], [0x01A0, 0x01EF]]
```

Run: `make exec CMD='cargo test -p device --test gputils_crosscheck'`
Expected: FAIL, naming `p16f877a` and printing both range lists.

- [ ] **Step 4: Prove the gate bites, using the `#88` falsification**

Now set the same file to the falsified map:

```toml
ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF], [0x0120, 0x016F], [0x0190, 0x01FF], [0x0200, 0x024F], [0x0280, 0x03FF]]
```

Run: `make exec CMD='cargo test -p device --test gputils_crosscheck'`
Expected: FAIL.

- [ ] **Step 5: Restore the file and confirm green**

```bash
git checkout crates/device/devices/p16f877a.toml
```

Run: `make exec CMD='cargo test -p device --test gputils_crosscheck'`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/device/tests/gputils_crosscheck.rs
git commit -m "test(device): cross-check every device ram map against gputils"
```

---

### Task 5: The flash bound probe

**Files:**
- Modify: `crates/device/tests/gputils_crosscheck.rs`

**Interfaces:**
- Consumes: `gputils_share` from Task 4, `device::Core`.
- Produces: nothing further.

- [ ] **Step 1: Write the failing test**

Append to `crates/device/tests/gputils_crosscheck.rs`:

```rust
use device::Core;
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

/// Assembles `org <addr>` for `dev` and returns gpasm's combined output.
/// gpasm exits 0 on a range warning, so the caller matches on the text.
fn probe_org(dev_name: &str, addr: u32, tag: &str) -> String {
    let dir = std::env::temp_dir();
    let asm = dir.join(format!("probe_{dev_name}_{tag}.asm"));
    let hex = dir.join(format!("probe_{dev_name}_{tag}.hex"));
    std::fs::write(&asm, format!("    org 0x{addr:X}\n    nop\n    end\n")).unwrap();
    let out = Command::new(gpasm())
        .args([
            "-p",
            dev_name,
            asm.to_str().unwrap(),
            "-o",
            hex.to_str().unwrap(),
        ])
        .output()
        .expect("gpasm must be runnable; set PIC8_GPASM");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const OVERFLOW: &str = "Address exceeds maximum range";

#[test]
fn flash_words_matches_gputils_for_every_device() {
    let Some(_share) = gputils_share() else { return };
    for dev in device::ALL {
        // org counts words on PIC14 and bytes on PIC18, so the last valid
        // address and the first bad one differ per core.
        let (last, past) = match dev.core {
            Core::Pic14 | Core::Pic14e => (dev.flash_words - 1, dev.flash_words),
            Core::Pic18 => (dev.flash_words * 2 - 2, dev.flash_words * 2),
        };

        let inside = probe_org(dev.name, last, "last");
        assert!(
            !inside.contains(OVERFLOW),
            "{}: gpasm rejects 0x{last:X}, which flash_words = {} claims exists:\n{inside}",
            dev.name,
            dev.flash_words
        );

        let outside = probe_org(dev.name, past, "past");
        assert!(
            outside.contains(OVERFLOW),
            "{}: gpasm accepts 0x{past:X}, past the {} words flash_words claims:\n{outside}",
            dev.name,
            dev.flash_words
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `make exec CMD='cargo test -p device --test gputils_crosscheck'`
Expected: PASS, both tests.

- [ ] **Step 3: Prove the flash gate bites**

Temporarily set `flash_words = 16384` in `crates/device/devices/p16f877a.toml`, the `#88` value.

Run: `make exec CMD='cargo test -p device --test gputils_crosscheck'`
Expected: FAIL on `flash_words_matches_gputils_for_every_device`, reporting that gpasm rejects the address `flash_words` claims exists.

- [ ] **Step 4: Restore and confirm green**

```bash
git checkout crates/device/devices/p16f877a.toml
```

Run: `make test CRATE=device`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/device/tests/gputils_crosscheck.rs
git commit -m "test(device): probe the flash bound against gpasm per core"
```

---

### Task 6: Teach the generator to emit provenance, and stop CI skipping silently

**Files:**
- Modify: `scripts/gen-device.py`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: the stanza shape from Task 1.
- Produces: generated TOMLs that satisfy `validate_provenance`.

- [ ] **Step 1: Find where the generator renders the TOML**

Run: `grep -n "def render\|def emit\|common_ram\|interrupt_vectors" scripts/gen-device.py | head -20`

Locate the function that writes the top-level scalar fields. The stanza is appended after them and before `[config]`.

- [ ] **Step 2: Emit the stanza**

In that renderer, after the `interrupt_vectors` line is written, append:

```python
    import hashlib
    digest = hashlib.sha256(atdf_path.read_bytes()).hexdigest()
    lines.append("")
    lines.append("[provenance]")
    lines.append('tier = "atdf"')
    lines.append(f'source = "{atdf_path.name}"')
    lines.append(f'pack = "{pack_name}"')
    lines.append(f'sha256 = "{digest}"')
```

`atdf_path` and `pack_name` are already resolved by the argument handling; if `pack_name` is not in scope, derive it from `atdf_path.parent.name`. Do not invent a pack string when one is available.

- [ ] **Step 3: Verify the generator still round-trips**

The DFP is not present locally, so this cannot be run end to end here. Confirm the file at least parses and the help still works:

Run: `python3 -m py_compile scripts/gen-device.py`
Expected: no output, meaning it parses.

Then run: `python3 scripts/gen-device.py --help`
Expected: the usage text, exit 0.

- [ ] **Step 4: Make the CI skip unmistakable**

In `.github/workflows/ci.yml`, the DFP step currently prints `DFP not installed on this runner, skipping strict check for $stem` and continues. Keep the behaviour, since the gputils gate is now the always-on one, but make the state legible. Replace the `else` branch body with:

```yaml
                echo "::warning::DFP absent, ATDF check SKIPPED for $stem. The always-on gate is crates/device/tests/gputils_crosscheck.rs"
```

- [ ] **Step 5: Run the full suite**

Run: `make test`
Expected: PASS, every crate.

- [ ] **Step 6: Commit**

```bash
git add scripts/gen-device.py .github/workflows/ci.yml
git commit -m "feat(device): emit provenance from the generator and surface skipped atdf checks"
```

---

### Task 7: Unit-test the generator against a synthetic ATDF

Microchip's own `.atdf` cannot be committed, so author a minimal one. This is
the only way the generator gets coverage without a DFP on the machine.

**Files:**
- Create: `scripts/fixtures/synthetic.atdf`
- Create: `scripts/test_gen_device.py`

**Interfaces:**
- Consumes: `scripts/gen-device.py`, including the provenance emission from Task 6.
- Produces: nothing later tasks rely on.

- [ ] **Step 1: Inspect what the generator expects to find**

Run: `grep -n "findall\|\.find(\|iter(\|tag ==\|attrib" scripts/gen-device.py | head -30`

Note the exact element and attribute names it reads for program space, RAM
regions and config fields. The fixture must use those names, not invented ones.

- [ ] **Step 2: Write the synthetic fixture**

Create `scripts/fixtures/synthetic.atdf` describing an imaginary tiny PIC14 part
with one 0x400-word program space, two GPR regions and one config byte, using
the element names found in Step 1. Keep it under 40 lines. Head it with a
comment noting it is hand-authored and describes no real silicon.

- [ ] **Step 3: Write the failing test**

Create `scripts/test_gen_device.py`:

```python
"""Generator coverage using a hand-authored ATDF. No vendor file is committed."""

import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
GEN = ROOT / "scripts" / "gen-device.py"
FIXTURE = ROOT / "scripts" / "fixtures" / "synthetic.atdf"


class GenDeviceTest(unittest.TestCase):
    def generate(self):
        with tempfile.TemporaryDirectory() as d:
            out = pathlib.Path(d) / "synthetic.toml"
            r = subprocess.run(
                [sys.executable, str(GEN), "synthetic",
                 "--atdf", str(FIXTURE), "--out", str(out)],
                capture_output=True, text=True,
            )
            self.assertEqual(r.returncode, 0, r.stderr)
            return out.read_text()

    def test_emits_a_provenance_stanza(self):
        text = self.generate()
        self.assertIn("[provenance]", text)
        self.assertIn('tier = "atdf"', text)
        self.assertIn("sha256 = ", text)

    def test_is_deterministic(self):
        self.assertEqual(self.generate(), self.generate())

    def test_emits_the_required_scalars(self):
        text = self.generate()
        for key in ("name", "core", "flash_words", "ram_banks"):
            self.assertIn(key, text)


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 4: Run it to verify it fails**

Run: `python3 scripts/test_gen_device.py -v`
Expected: FAIL, because the fixture does not yet satisfy the generator, or the
generator rejects the synthetic part name.

- [ ] **Step 5: Iterate the fixture until it passes**

Adjust `scripts/fixtures/synthetic.atdf` until the generator consumes it. Change
the fixture, not the generator, unless the generator has a genuine bug; if it
does, fix it and say so in the commit.

Run: `python3 scripts/test_gen_device.py -v`
Expected: PASS, 3 tests.

- [ ] **Step 6: Commit**

```bash
git add scripts/fixtures/synthetic.atdf scripts/test_gen_device.py
git commit -m "test(device): cover the generator with a synthetic atdf fixture"
```

---

### Task 8: The datasheet fallback prompt

The path for a part with neither an ATDF nor a `.lkr`. Its output is a proposal
for a human, never a trusted value.

**Files:**
- Create: `scripts/datasheet-extract.md`

**Interfaces:**
- Consumes: the stanza shape from Task 1.
- Produces: nothing code depends on.

- [ ] **Step 1: Write the prompt**

Create `scripts/datasheet-extract.md`:

```markdown
# Extracting a device TOML from a datasheet

Use this only when a part has neither a Microchip ATDF nor a gputils `.lkr`.
Both of those are machine readable and are checked automatically; a datasheet
reading is not, which is why its output is a proposal a human must confirm.

## Before you start

Confirm the cheap sources really are absent:

```bash
ls "$PIC8_GPUTILS_SHARE"/lkr/<stem>_g.lkr        # stem has no leading p
python3 scripts/gen-device.py <part> --check
```

If either works, stop and use it. This path is strictly the fallback.

## Extract

`pdftotext` is in the dev image. Convert with layout preserved, or the tables
become unreadable:

```bash
pdftotext -layout <datasheet>.pdf -
```

Find and quote, verbatim, the table that gives each of:

| Field | Table to look for |
|---|---|
| `ram_banks`, `common_ram` | the register file map, one row per bank, GPR ranges only |
| `flash_words` | program memory organization, in **words** for PIC14 and PIC18 |
| `stack_depth` | the hardware stack description |
| `interrupt_vectors` | the interrupt vector address |
| `config` | the configuration word, with each field's mask, shift and values |

## Rules

1. **Never infer a boundary.** If a table gives GPR as `0x20-0x6F`, that is the
   range. Do not round it, extend it to a bank edge, or copy a neighbouring
   part's value.
2. **SFR ranges are not GPR.** Only generally usable RAM belongs in `ram_banks`.
3. **Mirrored common RAM appears once**, in `common_ram`, not repeated per bank.
4. **Record where each number came from.** Every value needs a table reference.
5. If the datasheet is ambiguous, say so and stop. An ambiguous value that gets
   guessed is exactly the failure this path exists to avoid.

## Output

Emit the TOML with this stanza, and nothing invented:

```toml
[provenance]
tier = "datasheet"
document = "DS<number><rev>"
tables = ["<the exact table captions you used>"]
ticket = "epic-cc#<the ticket tracking this device>"
```

Then hand it to a human with the quoted tables alongside, so the numbers can be
checked without reopening the PDF. Do not commit it yourself.
```

- [ ] **Step 2: Verify it renders and has no em-dash**

Run: `grep -c "—" scripts/datasheet-extract.md`
Expected: `0`.

- [ ] **Step 3: Commit**

```bash
git add scripts/datasheet-extract.md
git commit -m "docs(device): prompt for datasheet extraction as the fallback path"
```

---

### Task 9: Distil the decision and sweep the plan

**Files:**
- Create: `docs/adr/ADR-021-device-provenance-and-cross-check.md`
- Modify: `docs/03-decisions.md`
- Delete: `docs/superpowers/plans/2026-08-23-device-provenance-gate.md`

- [ ] **Step 1: Write the ADR**

Create `docs/adr/ADR-021-device-provenance-and-cross-check.md`, following the shape of `ADR-020`:

```markdown
# ADR-021 -- Device data provenance and the gputils cross-check

**Status:** Accepted 2026-08-23<br>
**Decides:** `epic-cc#104`<br>
**Parent:** `docs/superpowers/specs/2026-08-23-device-provenance-design.md`

## Decision

* Every device TOML carries a required `[provenance]` table. `build.rs` refuses
  to build without it. `tier = "atdf"` needs `source`, `pack` and `sha256`;
  `tier = "datasheet"` needs `document` and `ticket`.
* `crates/device/tests/gputils_crosscheck.rs` gates every device on every PR:
  `flash_words` via two `gpasm` `org` probes, and the RAM map by comparing
  coalesced ranges against the generic `.lkr`.
* Comparison is union-and-coalesce, never element-wise, because gputils splits
  a flat PIC18 window into nine banks describing the same memory as one TOML
  entry.
* Missing gputils fails rather than skips, with `PIC8_ALLOW_NO_GPUTILS=1` as
  the explicit escape.

## Rationale

Device data was the only input with no oracle, and it sits upstream of banking,
allocation and paging, so an error there miscompiles silently. `#92` and `#101`
lost 32 bytes of GPR to a hand transcription; `#88` widened the part to hardware
that cannot exist and then rewrote every firewall that objected. The ATDF check
from ADR-020 cannot cover this on a PR, because it skips when the DFP is absent,
which is every GitHub runner. gputils ships in the dev image, so its check
cannot be skipped for want of a download.

## Alternatives rejected

* **Generate the TOML from gputils.** Cheapest, but makes the committed file a
  derivation of GPL data, which is the boundary `AGENTS.md` draws.
* **Fetch the DFP on every CI run.** Adds a network dependency to every build;
  a cache is an optimization in this repo, never a gate.
* **Probe gplink for RAM geometry.** Measured and rejected: absolute `udata` is
  never validated, and relocatable placement reveals no bank geometry. Supplying
  a linker script with the ranges would assert what we are verifying.

## Revisit if

* A supported part has no `.lkr`, making the cross-check silently vacuous for
  it. The datasheet provenance tier is the current answer, but a second oracle
  would be better.
```

- [ ] **Step 2: Add the index line**

In `docs/03-decisions.md`, next to the ADR-020 entry, add one line in the same format pointing at `ADR-021`.

- [ ] **Step 3: Delete the plan**

```bash
git rm docs/superpowers/plans/2026-08-23-device-provenance-gate.md
```

- [ ] **Step 4: Run the takeoff ritual**

Run: `make pre-pr-check PROSE=1 TEST=1`
Expected: ritual clean. Fix whatever it lists before continuing.

- [ ] **Step 5: Commit and open the PR**

```bash
git add docs/adr/ADR-021-device-provenance-and-cross-check.md docs/03-decisions.md
git commit -m "docs(device): ADR-021 for device provenance and the gputils cross-check"
git push -u origin feat/104-device-provenance
gh pr create --base master --title "feat(device): provenance stanza and an always-on gputils cross-check" --body "Closes #104"
```

Then: `epic-tasks review epic-cc#104 --pr <url>`
