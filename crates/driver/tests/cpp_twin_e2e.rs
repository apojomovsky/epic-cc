//! #461 oracle, secondary: C-twin differential. For the virtual and
//! RAII fixtures, the equivalent C program runs in the same sim and the
//! named globals must agree. Catches vtable-layout and cleanup-path bugs
//! hand asserts could miss. Dispatch sequences stay sim-only until the
//! `mdb` replay lands with #832 (docs/40 §5).

use std::collections::HashMap;
use std::process::Command;

fn map_addrs(map: &str) -> HashMap<String, usize> {
    map.lines()
        .filter_map(|l| {
            l.strip_prefix("global ")
                .and_then(|r| {
                    r.find(" 0x")
                        .map(|i| (r[..i].to_string(), r[i + 3..].trim()))
                })
                .and_then(|(n, a)| usize::from_str_radix(a, 16).ok().map(|v| (n, v)))
        })
        .collect()
}

fn run_hex(hex: &str) -> (Vec<u8>, bool) {
    let prog = pic14_sim::parse_hex_pic18(hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(200_000);
    (p.ram().to_vec(), p.halted())
}

fn compile_many(inputs: &[&str], extra: &[&str], tag: &str) -> (String, HashMap<String, usize>) {
    let hex_name = format!("tests/fixtures/cpp_twin_{tag}.hex");
    let map_name = format!("tests/fixtures/cpp_twin_{tag}.map");
    let mut args: Vec<&str> = inputs.to_vec();
    args.extend_from_slice(&["-o", &hex_name, "--map", &map_name, "--device", "p18f4550"]);
    args.extend_from_slice(extra);
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(&args)
        .output()
        .expect("run driver");
    assert!(
        output.status.success(),
        "driver failed on {inputs:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let map = std::fs::read_to_string(&map_name).expect("read map");
    let _ = std::fs::remove_file(&map_name);
    let hex = std::fs::read_to_string(&hex_name).unwrap();
    let _ = std::fs::remove_file(&hex_name);
    (hex, map_addrs(&map))
}

fn check_names(
    ram_cpp: &[u8],
    addrs_cpp: &HashMap<String, usize>,
    ram_c: &[u8],
    addrs_c: &HashMap<String, usize>,
    names: &[&str],
    what: &str,
) {
    for n in names.iter().copied() {
        let (a, b) = (addrs_cpp[n], addrs_c[n]);
        assert_eq!(
            ram_cpp[a], ram_c[b],
            "{what}: {n} differs (C++={} C={})",
            ram_cpp[a], ram_c[b]
        );
    }
}

#[test]
fn virtual_dispatch_matches_c_twin() {
    for sel in [0u8, 1, 2] {
        let flag = format!("SEL={sel}");
        let (hex_cpp, map_cpp) = compile_many(
            &["tests/fixtures/cpp_virtual.cpp"],
            &["-D", &flag],
            &format!("virt_cpp_{sel}"),
        );
        let (hex_c, map_c) = compile_many(
            &["tests/fixtures/cpp_virtual_twin.c"],
            &["-D", &flag],
            &format!("virt_c_{sel}"),
        );
        let (ram_cpp, halt_cpp) = run_hex(&hex_cpp);
        let (ram_c, halt_c) = run_hex(&hex_c);
        assert!(halt_cpp && halt_c, "both must halt (sel={sel})");
        check_names(
            &ram_cpp,
            &map_cpp,
            &ram_c,
            &map_c,
            &["in_sel", "out_val"],
            "virtual",
        );
    }
}

#[test]
fn raii_cleanup_matches_c_twin() {
    let (hex_cpp, map_cpp) = compile_many(&["tests/fixtures/cpp_raii_sim.cpp"], &[], "raii_cpp");
    let (hex_c, map_c) = compile_many(&["tests/fixtures/cpp_raii_twin.c"], &[], "raii_c");
    let (ram_cpp, halt_cpp) = run_hex(&hex_cpp);
    let (ram_c, halt_c) = run_hex(&hex_c);
    assert!(halt_cpp && halt_c, "both must halt");
    for (base, len) in [("out_vals", 3), ("g_log", 4)] {
        for i in 0..len {
            let (a, b) = (map_cpp[base] + i, map_c[base] + i);
            assert_eq!(
                ram_cpp[a], ram_c[b],
                "raii: {base}[{i}] differs (C++={} C={})",
                ram_cpp[a], ram_c[b]
            );
        }
    }
    check_names(
        &ram_cpp,
        &map_cpp,
        &ram_c,
        &map_c,
        &["out_n", "g_n"],
        "raii",
    );
}
