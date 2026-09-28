//! `#pragma config` lowering (epic-cc#689): clang drops unknown pragmas
//! before the `.ll`, so the driver recovers them from the raw sources and
//! lowers them through the same resolution `EPIC_CONFIG` uses.

use std::process::Command;

fn driver() -> Command {
    Command::new(env!("CARGO_BIN_EXE_epic-cc"))
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
        + &String::from_utf8_lossy(&out.stdout).to_string()
}

fn write_c(tag: &str, body: &str) -> (String, String) {
    let dir = std::env::temp_dir().join(format!("epic-pragma-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let c_path = dir.join("prog.c");
    std::fs::write(&c_path, body).unwrap();
    let hex_path = dir.join("prog.hex");
    (c_path.display().to_string(), hex_path.display().to_string())
}

/// The same config once as `EPIC_CONFIG` (normalized names) and once as
/// `#pragma config` (pack-native names): both must resolve the identical
/// config bytes, fields, and clock, proving one shared resolution. (Whole
/// HEX cannot match: the `EPIC_CONFIG` twin carries its `.epiccfg` section
/// global as an extra flash const.)
#[test]
fn pragma_config_matches_epic_config_resolution() {
    const BODY: &str = "volatile unsigned char g;\nvoid main(void) { g = 1; }\n";
    let (epic_c, epic_hex) = write_c(
        "epic",
        &format!("#include <epic-cc.h>\nEPIC_CONFIG(\"osc=intio, plldiv=div5, cpudiv=div1, usbdiv=on\");\n{BODY}"),
    );
    let (pragma_c, pragma_hex) = write_c(
        "pragma",
        &format!(
            "#pragma config FOSC = INTOSCIO_EC, PLLDIV = 5\n#pragma config CPUDIV = OSC1_PLL2, USBDIV = 2\n{BODY}"
        ),
    );
    let epic_report = format!("{epic_hex}.json");
    let pragma_report = format!("{pragma_hex}.json");
    for (c, hex, report) in [
        (&epic_c, &epic_hex, &epic_report),
        (&pragma_c, &pragma_hex, &pragma_report),
    ] {
        let out = driver()
            .args([c, "-o", hex, "--device", "p18f4550", "--report", report])
            .output()
            .expect("run driver");
        assert!(
            out.status.success(),
            "driver failed for {c}: {}",
            stderr_of(&out)
        );
        assert!(
            stderr_of(&out).contains("resolved configuration for p18f4550"),
            "stderr: {}",
            stderr_of(&out)
        );
    }
    for key in ["\"clock_hz\"", "\"bytes\"", "\"fields\""] {
        let line_of = |path: &str| {
            std::fs::read_to_string(path)
                .expect("read report")
                .lines()
                .filter(|l| l.contains(key))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(
            line_of(&epic_report),
            line_of(&pragma_report),
            "one resolution means one {key}"
        );
    }
    let _ = std::fs::remove_dir_all(std::path::Path::new(&epic_c).parent().unwrap());
    let _ = std::fs::remove_dir_all(std::path::Path::new(&pragma_c).parent().unwrap());
}

#[test]
fn pragma_and_epic_config_mixing_is_an_error() {
    let (c, hex) = write_c(
        "mix",
        "EPIC_CONFIG(\"osc=xt, wdt=off\");\n#pragma config FOSC = XT\nvoid main(void) {}\n",
    );
    let out = driver()
        .args([c.as_str(), "-o", hex.as_str(), "--device", "p16f877a"])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&c).parent().unwrap());
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.contains("cannot be mixed"), "{err}");
    assert!(err.contains("EPIC_CONFIG"), "{err}");
}

#[test]
fn unknown_pragma_field_lists_the_valid_names() {
    let (c, hex) = write_c("badfield", "#pragma config WAT = OFF\nvoid main(void) {}\n");
    let out = driver()
        .args([c.as_str(), "-o", hex.as_str(), "--device", "p16f877a"])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&c).parent().unwrap());
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.contains("unknown config field 'WAT'"), "{err}");
    assert!(err.contains("expected one of:"), "{err}");
    assert!(err.contains("osc"), "{err}");
}

#[test]
fn unknown_pragma_value_lists_the_valid_values() {
    let (c, hex) = write_c(
        "badvalue",
        "#pragma config FOSC = TURBO\nvoid main(void) {}\n",
    );
    let out = driver()
        .args([c.as_str(), "-o", hex.as_str(), "--device", "p16f877a"])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&c).parent().unwrap());
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.contains("unknown config value 'TURBO'"), "{err}");
    assert!(err.contains("expected one of:"), "{err}");
}

#[test]
fn pragma_omitted_oscillator_tree_fields_stay_erased() {
    // epic-cc#706 option 2: the program from the issue omits the 4550's
    // defaultless usbdiv/cpudiv/plldiv, which XC8 leaves erased. The
    // resolved bytes must equal the erased baseline except where set.
    let (c, hex) = write_c(
        "defaults",
        "#pragma config FOSC = INTOSCIO_EC, WDT = OFF, LVP = OFF, XINST = OFF\nvoid main(void) {}\n",
    );
    let out = driver()
        .args([c.as_str(), "-o", hex.as_str(), "--device", "p18f4550"])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&c).parent().unwrap());
    let err = stderr_of(&out);
    assert!(out.status.success(), "driver failed: {err}");
    assert!(err.contains("byte 0x300000 = 0xFF"), "{err}");
    assert!(err.contains("byte 0x300001 = 0x38"), "{err}");
}
