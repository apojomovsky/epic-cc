//! #457 acceptance: the driver compiles a `.cpp` input as C++ (`-x c++`
//! with `-fno-exceptions -fno-rtti`) and the lone mangled `_Z4mainv` entry
//! reaches the backend as `main`. The fixture stays class-free on purpose:
//! C++ IR shapes (`%class`, aliases, param attrs) belong to #799. PIC18
//! only, per the EC++ epic scope.

use std::process::Command;

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn cpp_min_program_runs_on_pic18() {
    let fixture = "tests/fixtures/cpp_min.cpp";
    let hex_name = "tests/fixtures/cpp_min_p18f4550.hex";
    let map_name = "tests/fixtures/cpp_min_p18f4550.map";
    // The input rides in as a real initializer (`__start` clears
    // zero-initialized globals before main, epic-cc#561, which would erase
    // a sim-side seed).
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            fixture, "-o", hex_name, "--map", map_name, "--device", "p18f4550", "-D", "IN_VAL=7",
        ])
        .output()
        .expect("run driver");
    assert!(
        output.status.success(),
        "driver failed on {fixture}: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let map = std::fs::read_to_string(map_name).expect("read map");
    let _ = std::fs::remove_file(map_name);
    let addr = |n: &str| map_addr(&map, n);

    let hex = std::fs::read_to_string(hex_name).unwrap();
    let _ = std::fs::remove_file(hex_name);
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(200_000);
    assert!(p.halted(), "Pic18 sim should halt");
    assert_eq!(p.ram()[addr("out_val")], 0x08, "out = in + 1");
}

#[test]
fn cpp_exceptions_rejected_by_frontend() {
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/cpp_exceptions_reject.cpp",
            "-o",
            "tests/fixtures/cpp_exceptions_reject.hex",
            "--device",
            "p18f4550",
        ])
        .output()
        .expect("run driver");
    assert!(!output.status.success(), "try/catch must not compile");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("exceptions disabled"),
        "frontend error names the rule: {stderr}"
    );
}
