//! epic-cc#144 regression: a NULL function argument compiles and runs.
//! clang -O1 prints `ptr noundef null` for a NULL pointer argument;
//! irparse's call-arg whitelist accepted poison but not null, so the
//! build panicked `call arg must carry a value`. parse_val already maps
//! null to Const(0), so the arg parser just had to forward it.

use std::process::Command;

/// `g_out`'s RAM address, read off the compiler's own `--map`
/// output. Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts: the PIC18 path alone parses with
/// switches preserved, and once the frames sit below the globals a
/// difference that far upstream moves every global address.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn null_function_arg_compiles_and_runs() {
    let hex_path = std::env::temp_dir().join(format!("null_arg_{}.hex", std::process::id()));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F877A", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg("tests/fixtures/null_arg.c")
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read hex");
    let _ = std::fs::remove_file(&hex_path);
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&map_path);
    let out_addr = map_addr(&map, "g_out");
    let prog = pic14_sim::parse_hex(&produced);
    let mut sim = pic14_sim::Pic14::new(prog);
    sim.run(50_000);
    assert_eq!(sim.ram()[out_addr], 7, "g_out == 7 for a NULL callback");
}
