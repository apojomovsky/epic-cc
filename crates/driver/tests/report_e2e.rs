//! The `--report` JSON (epic-cc#693): what the PlatformIO builder reads for
//! its size bar and its pre-flash checks.

use driver::report::decode_config;
use std::process::Command;

fn tmp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("epic-cc-report-{}-{name}", std::process::id()))
}

/// `tag` keeps each test's files apart: the tests run as threads of one
/// process, so the pid alone would let one read another's report.
fn build_report(tag: &str, source: &str, device: &str) -> String {
    let (hex, json) = (tmp(&format!("{tag}.hex")), tmp(&format!("{tag}.json")));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([source, "-o", hex.to_str().unwrap(), "--device", device])
        .args(["--report", json.to_str().unwrap()])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&json).expect("report written");
    let _ = std::fs::remove_file(&hex);
    let _ = std::fs::remove_file(&json);
    text
}

#[test]
fn a_configured_build_reports_its_resolved_fields_and_clock() {
    let json = build_report("configured", "tests/fixtures/config_probe.c", "p16f877a");
    assert!(json.contains("\"device\": \"p16f877a\""), "{json}");
    assert!(json.contains("\"total\": 8192"), "{json}");
    assert!(json.contains("\"source\": \"program\""), "{json}");
    assert!(json.contains("\"lvp\": \"off\""), "{json}");
    assert!(json.contains("\"clock_hz\": 4000000"), "{json}");
    assert!(json.contains("\"version\": 1"), "{json}");
}

#[test]
fn an_unconfigured_build_reports_the_erased_configuration() {
    // No EPIC_CONFIG: the HEX has no config word, so the part keeps its
    // erased 0x3FFF, where LVP is on (DS39582C Register 14-1, bit 7).
    let json = build_report("erased", "tests/fixtures/add.c", "p16f877a");
    assert!(json.contains("\"source\": \"erased\""), "{json}");
    assert!(json.contains("\"bytes\": [255, 63]"), "{json}");
    assert!(json.contains("\"lvp\": \"on\""), "{json}");
    assert!(json.contains("\"clock_hz\": null"), "{json}");
}

#[test]
fn an_unconfigured_pic18_build_claims_no_configuration() {
    // The PIC18 baseline is gpasm's fill, not the erased silicon, so the
    // report must not decode it as the part's state.
    let json = build_report("pic18", "tests/fixtures/add.c", "p18f4550");
    assert!(json.contains("\"source\": \"unset\""), "{json}");
    assert!(json.contains("\"bytes\": null"), "{json}");
    assert!(json.contains("\"xinst\": null"), "{json}");
}

#[test]
fn decoding_matches_values_through_a_scattered_mask() {
    // 628A `osc` owns mask 0x13 (FOSC<2> sits at bit 4), so a shift alone
    // cannot recover its value; every value must round-trip anyway.
    let region = &device::PIC16F628A.config;
    let osc = region.fields.iter().find(|f| f.name == "osc").unwrap();
    assert_eq!(osc.mask, 0x13);
    for v in osc.values {
        let bytes = device::resolve_config(region, &format!("osc={}, pwrt=on", v.name));
        let decoded = decode_config(region, &bytes);
        assert_eq!(
            decoded.iter().find(|(n, _)| *n == "osc").unwrap().1,
            Some(v.name)
        );
    }
}
