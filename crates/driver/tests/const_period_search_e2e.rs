//! epic-cc#1212: an all-constant prescaler search must fold to
//! constant stores at compile time, not run a u32 divide loop at
//! boot. The fixture mirrors `compute_period` in epic-hal's tick
//! module (3x16x2 search, only caller passes 20 MHz). Acceptance:
//! (a) the emitted asm calls no u32 divide or multiply routine, and
//! (b) the sim still observes the searched result (249, 1, 4).

use std::process::Command;

fn tmp(name: &str, ext: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "const_period_search_{name}_{}.{ext}",
        std::process::id()
    ))
}

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn const_period_search_folds_and_still_computes() {
    let asm_path = tmp("main", "asm");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/const_period_search.c",
            "--emit",
            "asm",
            "-o",
        ])
        .arg(&asm_path)
        .output()
        .expect("run driver for asm");
    assert!(
        out.status.success(),
        "driver asm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let asm = std::fs::read_to_string(&asm_path).expect("read emitted asm");
    let _ = std::fs::remove_file(&asm_path);
    assert!(
        !asm.contains("__udiv_u32") && !asm.contains("__mul_u32"),
        "constant search still calls a u32 divide/multiply routine:\n{asm}"
    );

    let hex_path = tmp("main", "hex");
    let map_path = tmp("main", "map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/const_period_search.c", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .output()
        .expect("run driver for hex");
    assert!(
        out.status.success(),
        "driver hex: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).expect("read hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(100_000);
    assert_eq!(p.ram()[map_addr(&map, "out_pr2")], 249, "PR2 for 20 MHz");
    assert_eq!(p.ram()[map_addr(&map, "out_pre")], 1, "prescaler enum");
    assert_eq!(p.ram()[map_addr(&map, "out_post")], 4, "postscaler");
}
