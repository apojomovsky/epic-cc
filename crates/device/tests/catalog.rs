//! The catalog gate. The catalog (`catalog/parts.toml`) is roadmap data per
//! docs/38 D-3, not compiler input, so the gate guards what consumers rely
//! on: F-generation scope, column shape, tier consistency, and agreement
//! with the shipped device registry the catalog claims to track.

use std::collections::BTreeSet;
use std::path::Path;

fn catalog() -> toml::Value {
    let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/catalog/parts.toml"))
        .expect("read catalog/parts.toml");
    toml::from_str(&raw).expect("catalog/parts.toml parses")
}

fn parts() -> Vec<toml::Value> {
    catalog()
        .get("part")
        .and_then(|v| v.as_array())
        .cloned()
        .expect("catalog has [[part]] rows")
}

fn str_field(row: &toml::Value, key: &str) -> String {
    row.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("part missing {key}: {row:?}"))
        .to_string()
}

fn is_flash_generation(display: &str) -> bool {
    // Flash families with their low-voltage (LF) and high-voltage (HV)
    // orderable variants; the EPROM/OTP C parts never match.
    [
        "PIC10F", "PIC10LF", "PIC12F", "PIC12LF", "PIC12HV", "PIC16F", "PIC16LF", "PIC16HV",
        "PIC18F", "PIC18LF",
    ]
    .iter()
    .any(|prefix| display.starts_with(prefix))
}

#[test]
fn every_row_is_flash_generation_with_valid_shape() {
    let rows = parts();
    assert!(
        rows.len() > 900,
        "catalog unexpectedly small: {}",
        rows.len()
    );
    let mut names = BTreeSet::new();
    let mut displays = BTreeSet::new();
    for row in &rows {
        let display = str_field(row, "display");
        let name = str_field(row, "name");
        assert!(is_flash_generation(&display), "{display} is not an F part");
        assert!(
            name.starts_with('p') && name.to_lowercase() == name,
            "{name} is not a canonical lowercase stem"
        );
        assert_eq!(
            format!("PIC{}", name[1..].to_uppercase()),
            display,
            "name and display disagree for {display}"
        );
        assert!(names.insert(name), "duplicate name in catalog");
        assert!(displays.insert(display.clone()), "duplicate display");

        let core = str_field(row, "core");
        assert!(
            ["pic-baseline", "pic14", "pic14e", "pic18"].contains(&core.as_str()),
            "{display}: unknown core {core}"
        );
        let flash = row
            .get("flash_words")
            .and_then(|v| v.as_integer())
            .unwrap_or_else(|| panic!("{display} has no flash_words"));
        assert!(flash >= 64, "{display}: implausible flash {flash}");
        if let Some(ram) = row.get("ram_bytes").and_then(|v| v.as_integer()) {
            assert!(ram > 0, "{display}: non-positive ram_bytes");
        }
        if let Some(ee) = row.get("eeprom_bytes").and_then(|v| v.as_integer()) {
            assert!(ee >= 0, "{display}: negative eeprom_bytes");
        }
        let page = str_field(row, "page");
        assert!(
            page.starts_with("https://www.microchip.com/en-us/product/"),
            "{display}: unexpected product page {page}"
        );
        if let Some(pdf) = row.get("pdf").and_then(|v| v.as_str()) {
            assert!(
                pdf.starts_with("https://"),
                "{display}: datasheet url must be absolute"
            );
        }
        if let Some(pack) = row.get("dfp").and_then(|v| v.as_str()) {
            assert!(pack.starts_with("Microchip."), "{display}: pack {pack}");
        }
        let tier = str_field(row, "tier");
        let scored = row.get("gh_refs").and_then(|v| v.as_integer());
        match (tier.as_str(), scored) {
            ("unscored", None) => {}
            ("high" | "mid" | "low", Some(refs)) => {
                let expected = if refs >= 5000 {
                    "high"
                } else if refs >= 500 {
                    "mid"
                } else {
                    "low"
                };
                assert_eq!(tier, expected, "{display}: tier does not match refs");
            }
            other => panic!("{display}: tier/gh_refs inconsistent: {other:?}"),
        }
    }
}

#[test]
fn family_prefix_matches_core() {
    for row in parts().iter() {
        let display = str_field(row, "display");
        let core = str_field(row, "core");
        if display.starts_with("PIC18") {
            assert_eq!(core, "pic18", "{display} must be pic18");
        }
        if ["PIC10F2", "PIC12F5", "PIC16F5"]
            .iter()
            .any(|p| display.starts_with(p))
        {
            assert_eq!(core, "pic-baseline", "{display} must be pic-baseline");
        }
    }
}

#[test]
fn every_shipped_device_is_tracked_with_matching_facts() {
    let rows = parts();
    let by_name: BTreeSet<String> = rows.iter().map(|row| str_field(row, "name")).collect();
    let mut checked = 0;
    let devices_dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/devices"));
    for entry in std::fs::read_dir(devices_dir).expect("devices dir") {
        let path = entry.expect("device file").path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let raw = std::fs::read_to_string(&path).expect("read device toml");
        let toml: toml::Value = toml::from_str(&raw).expect("parse device toml");
        let name = toml.get("name").and_then(|v| v.as_str()).expect("name");
        assert!(
            by_name.contains(name),
            "{name} is shipped in devices/ but absent from the catalog"
        );
        let row = rows
            .iter()
            .find(|row| str_field(row, "name") == name)
            .expect("catalog row for shipped device");
        assert_eq!(
            str_field(row, "core"),
            toml.get("core").and_then(|v| v.as_str()).expect("core"),
            "{name}: catalog core disagrees with the shipped TOML"
        );
        assert_eq!(
            row.get("flash_words").and_then(|v| v.as_integer()),
            toml.get("flash_words").and_then(|v| v.as_integer()),
            "{name}: catalog flash disagrees with the shipped TOML"
        );
        checked += 1;
    }
    assert!(checked >= 10, "device registry unexpectedly small");
}
