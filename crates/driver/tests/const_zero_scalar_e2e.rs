//! epic-cc#557: a `const` scalar whose value is zero still needs its table
//! byte. Before this, irparse dropped an all-zero initializer, so the table
//! emitter's `!bytes.is_empty()` assert fired: `const unsigned char tab = 0;`
//! panicked with "isel: const @tab has no table bytes" on every backend.
//! (Before epic-cc#454 the same panic hit every const scalar, zero or not.)
//!
//! The read is observable: `out` receives the table byte, which must be 0.

use std::process::Command;

fn compile_and_run(device: &str) -> (pic14_sim::Pic18, String) {
    let hex = std::env::temp_dir().join(format!("const0_{device}-{}.hex", std::process::id()));
    let map = std::env::temp_dir().join(format!("const0_{device}-{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/const_zero_scalar.c", "-o"])
        .arg(&hex)
        .arg("--map")
        .arg(&map)
        .args(["--device", device])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device} must compile a zero const scalar: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&map).expect("read map");
    let prog = pic14_sim::parse_hex_pic18(&std::fs::read_to_string(&hex).expect("read hex"));
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(200_000);
    (p, text)
}

fn mapped(map: &str, name: &str) -> usize {
    map.lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("global ")
                .and_then(|r| r.split_once(' '))
                .filter(|(n, _)| *n == name)
                .and_then(|(_, a)| usize::from_str_radix(a.trim_start_matches("0x"), 16).ok())
        })
        .unwrap_or_else(|| panic!("{name} not in map:\n{map}"))
}

#[test]
fn a_zero_valued_const_scalar_compiles_and_reads_zero() {
    let (p, map) = compile_and_run("18F4550");
    assert!(p.halted(), "program must halt");
    assert_eq!(
        p.ram()[mapped(&map, "out")],
        0,
        "the table byte for `const unsigned char tab = 0` is 0"
    );
}

#[test]
fn a_zero_valued_const_scalar_compiles_on_pic16() {
    let hex = std::env::temp_dir().join(format!("const0_16-{}.hex", std::process::id()));
    let map = std::env::temp_dir().join(format!("const0_16-{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/const_zero_scalar.c", "-o"])
        .arg(&hex)
        .arg("--map")
        .arg(&map)
        .args(["--device", "16F877A"])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver 16F877A must compile a zero const scalar: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&map).expect("read map");
    let prog = pic14_sim::parse_hex(&std::fs::read_to_string(&hex).expect("read hex"));
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(200_000);
    assert!(p.halted(), "program must halt");
    assert_eq!(
        p.ram()[mapped(&text, "out")],
        0,
        "the RETLW table byte for `const unsigned char tab = 0` is 0"
    );
}
