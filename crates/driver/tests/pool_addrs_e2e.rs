//! Pool addresses and RAM-copy gating (epic-cc#816): with
//! `--const-pool` the direct-arg string consts (the encoder-sim log
//! shape) carry no RAM copies and materialize as pool addresses, while
//! the mixed-use const keeps its copy. Unflagged output keeps every
//! copy. The sim behavior proof itself lands in epic-cc#817.

use std::process::Command;

fn build(tag: &str, extra: &[&str]) -> (String, String) {
    // Unique paths per test: the suite runs tests in parallel and two
    // tests sharing one temp file delete each other's output.
    let dir = std::env::temp_dir();
    let asm_path = dir.join(format!("pool_addrs_{tag}_{}.asm", std::process::id()));
    let map_path = dir.join(format!("pool_addrs_{tag}_{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/pool_addrs_mixed.c", "-o"])
        .arg(&asm_path)
        .args(["--device", "p16f877a", "--emit", "asm", "--map"])
        .arg(&map_path)
        .args(extra)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver failed {tag}:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let asm = std::fs::read_to_string(&asm_path).unwrap();
    let map = std::fs::read_to_string(&map_path).unwrap();
    let _ = std::fs::remove_file(&asm_path);
    let _ = std::fs::remove_file(&map_path);
    (asm, map)
}

fn globals(map: &str) -> Vec<&str> {
    map.lines()
        .filter_map(|l| l.strip_prefix("global "))
        .map(|l| l.split_whitespace().next().unwrap_or(""))
        .collect()
}

fn consts(map: &str) -> Vec<&str> {
    map.lines()
        .filter_map(|l| l.strip_prefix("const "))
        .map(|l| l.split_whitespace().next().unwrap_or(""))
        .collect()
}

fn staged(map: &str) -> Vec<&str> {
    map.lines()
        .filter_map(|l| l.strip_prefix("staged "))
        .map(|l| l.split_whitespace().next().unwrap_or(""))
        .collect()
}

#[test]
fn pool_addrs_gate_copies_and_rewrite_addresses() {
    let (asm, map) = build("flagged", &["--const-pool"]);
    let gs = globals(&map);
    let cs = consts(&map);
    assert!(
        gs.contains(&"mixed"),
        "mixed-use const keeps its RAM copy: {gs:?}"
    );
    assert!(
        !gs.iter().any(|g| g.starts_with(".str")),
        "direct-only string consts carry no RAM copies: {gs:?}"
    );
    assert!(
        cs.iter().filter(|c| c.starts_with(".str")).count() == 2,
        "both string literals stay flash consts: {cs:?}"
    );
    assert!(
        !cs.contains(&"mixed"),
        "mixed-use const leaves the flash set: {cs:?}"
    );
    assert!(
        asm.contains("__const_pool:"),
        "pool chunk is emitted:\n{asm}"
    );
    assert!(
        asm.contains("LOW(__const_pool"),
        "direct const args materialize pool addresses:\n{asm}"
    );
    assert!(
        staged(&map).is_empty() && !gs.contains(&"__const_stage"),
        "nothing stages once copies gate: {map}"
    );
}

#[test]
fn pool_addrs_unflagged_stages_and_copies() {
    let (_, map) = build("unflagged", &[]);
    let gs = globals(&map);
    let ss = staged(&map);
    assert!(
        gs.contains(&"mixed"),
        "mixed-use const copies without the flag: {gs:?}"
    );
    assert!(
        ss.iter().filter(|s| s.starts_with(".str")).count() == 2 && gs.contains(&"__const_stage"),
        "string literals stage without the flag: {map}"
    );
}
