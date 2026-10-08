//! epic-cc#778: four dynamically indexed const bytes sharing one seed.
//!
//! Compiles `fixtures/const_dynrun.c` through the real driver (default
//! pipeline, outlining on) and asserts the acceptance shape end to end:
//! the asm keeps exactly one `SCRIPT` seed for all four reads walking
//! `TBLRD*+`, and the sim lands the right bytes. The index is harness
//! owned: `__start` clears it, so the test steps past the clear loop
//! (whose take the listing names) before poking, then runs to halt.

use std::process::Command;

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn tmp(tag: &str, ext: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "const_dynrun_{}_{}_{}",
        std::process::id(),
        tag,
        ext
    ))
}

fn asm_for() -> String {
    let path = tmp("main", "asm");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "18F4550", "--emit", "asm", "-o"])
        .arg(&path)
        .arg("tests/fixtures/const_dynrun.c")
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let asm = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    asm
}

/// Steps from `__start` to main's entry: the zero-loop take plus the
/// `call main`. Asserts the whole window shape (not just the take), so a
/// future nonzero init or allocator change fails here naming this helper
/// instead of landing the poke mid-init.
fn start_steps_to_main(asm: &str) -> usize {
    let window = asm.split("__start:").nth(1).expect("__start label");
    let mut lines = window.lines().map(str::trim).filter(|l| !l.is_empty());
    assert!(
        lines.next().is_some_and(|l| l.starts_with("LFSR 0, 0x")),
        "clear base:\n{asm}"
    );
    let take = lines
        .next()
        .and_then(|l| l.strip_prefix("MOVLW 0x"))
        .expect("clear take");
    let take = usize::from(u8::from_str_radix(take, 16).expect("hex take"));
    assert!(
        lines.next().is_some_and(|l| l.ends_with(':')),
        "loop label:\n{asm}"
    );
    assert_eq!(lines.next(), Some("CLRF 0xFEE,A"), "clear body:\n{asm}");
    assert_eq!(
        lines.next(),
        Some("DECFSZ 0xFE8,F,A"),
        "clear count:\n{asm}"
    );
    assert!(
        lines.next().is_some_and(|l| l.starts_with("BRA ")),
        "loop back:\n{asm}"
    );
    assert_eq!(lines.next(), Some("call main"), "entry call:\n{asm}");
    3 * take + 2
}

#[test]
fn dynamic_const_run_reads_correct_bytes() {
    let asm = asm_for();
    // One seed for the whole listing: the outliner may house the walks in
    // a shared helper, but a single `SCRIPT` seed plus sim-correct bytes
    // proves the run walks instead of reseeding whatever the factoring.
    assert_eq!(
        asm.matches("MOVLW LOW(SCRIPT)").count(),
        1,
        "one run seeds once:\n{asm}"
    );

    let hex_path = tmp("main", "hex");
    let map_path = tmp("main", "map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/const_dynrun.c", "--target", "18F4550"])
        .arg("-o")
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).expect("read hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    let idx = map_addr(&map, "g_idx");
    let tick = map_addr(&map, "g_tick");
    let event = map_addr(&map, "g_event");

    // The poke must land after `__start` zeroed the globals but before
    // main reads the index in.
    let steps = start_steps_to_main(&asm);
    let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
    for _ in 0..steps {
        p.step();
    }
    p.ram_mut()[idx] = 3;
    p.run(200_000);
    assert!(p.halted(), "program must halt");
    let ram = p.ram();
    let tick_v = u16::from_le_bytes([ram[tick], ram[tick + 1]]);
    let event_v = u16::from_le_bytes([ram[event], ram[event + 1]]);
    assert_eq!(tick_v, 20, "SCRIPT[3].tick");
    assert_eq!(event_v, 4, "SCRIPT[3].event");
}
