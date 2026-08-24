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
