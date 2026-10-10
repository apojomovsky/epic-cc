//! The ticket's two-function program (epic-cc#1036): functions named `endpoint`
//! and `listen` start with assembler directive names. Without `noinline` both
//! are inlined into `main`, so this exercises the inlined path that
//! `directive_prefix_names_e2e.rs` (which uses `noinline`) does not.
//! Acceptance: compiles, and the simulator leaves `p == 2` (last write wins).
//!
//! `p`'s RAM address is read off the compiler's own `--map` output.

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
fn ticket_two_function_program_runs_correctly() {
    let pid = std::process::id();
    let dir = std::env::temp_dir();
    let src = dir.join(format!("epic-label-prefix-{pid}.c"));
    let hex = dir.join(format!("epic-label-prefix-{pid}.hex"));
    let map = dir.join(format!("epic-label-prefix-{pid}.map"));
    std::fs::write(
        &src,
        "volatile unsigned char p;\n\
         void endpoint(void) { p = 1; }\n\
         void listen(void) { p = 2; }\n\
         void main(void) { endpoint(); listen(); }\n",
    )
    .expect("write temp C file");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            src.display().to_string().as_str(),
            "-o",
            hex.display().to_string().as_str(),
            "--map",
            map.display().to_string().as_str(),
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    let hex_text = std::fs::read_to_string(&hex).unwrap_or_default();
    let map_text = std::fs::read_to_string(&map).unwrap_or_default();
    for f in [&src, &hex, &map] {
        let _ = std::fs::remove_file(f);
    }
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("internal compiler error"), "{err}");
    let p_addr = map_addr(&map_text, "p");

    let mut sim = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex_text));
    sim.run(200_000);
    assert!(sim.halted());
    assert_eq!(
        sim.ram()[p_addr],
        2,
        "listen() runs after endpoint(); p == 2"
    );
}
