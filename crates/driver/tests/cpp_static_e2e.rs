//! #458 acceptance: global constructors run before `main`
//! (`llvm.global_ctors`), `__cxa_atexit` calls are erased, and a
//! function-local static initializes once through its trivialized guard.
//! PIC18 only, per the EC++ epic scope.

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
fn cpp_static_init_runs_before_main_on_pic18() {
    let fixture = "tests/fixtures/cpp_static.cpp";
    let hex_name = "tests/fixtures/cpp_static_p18f4550.hex";
    let map_name = "tests/fixtures/cpp_static_p18f4550.map";
    // The input rides in as a real initializer (`__start` clears
    // zero-initialized globals before main, epic-cc#561).
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
    // out_global = ctor value 9 + in 7; out_local = guarded static 9.
    assert_eq!(p.ram()[addr("out_global")], 16, "global ctor ran");
    assert_eq!(p.ram()[addr("out_local")], 9, "local static ran once");
}

#[test]
fn cpp_isr_first_touch_of_local_static_fails_loudly() {
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/cpp_isr_guard_reject.cpp",
            "-o",
            "tests/fixtures/cpp_isr_guard_reject.hex",
            "--device",
            "p18f4550",
        ])
        .output()
        .expect("run driver");
    assert!(!output.status.success(), "ISR first-touch must not compile");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("init-once guard cannot be trusted"),
        "panic names the rule: {stderr}"
    );
}
