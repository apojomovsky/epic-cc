//! #461 corpus: C++ `struct` plus `extern "C"` across two TUs. The
//! `.cpp` and `.c` units merge through the unchanged `llvm-link` path;
//! the test pins the shared struct layout and the cross-language call.
//! Hand asserts in the `Pic18` sim (primary oracle).

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
fn cpp_struct_and_extern_c_mix() {
    let hex_name = "tests/fixtures/cpp_mix_p18f4550.hex";
    let map_name = "tests/fixtures/cpp_mix_p18f4550.map";
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/cpp_mix.cpp",
            "tests/fixtures/cpp_mix_c.c",
            "-o",
            hex_name,
            "--map",
            map_name,
            "--device",
            "p18f4550",
        ])
        .output()
        .expect("run driver");
    assert!(
        output.status.success(),
        "driver failed on mix: {}",
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
    assert_eq!(p.ram()[addr("out_sum")], 7, "c_add(3, 4)");
    assert_eq!(p.ram()[addr("out_first")], 3, "c_add(3, 0)");
}
