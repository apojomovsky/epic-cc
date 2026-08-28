//! epic-cc#151 regression: the const-table section start must be measured
//! at the assembler's final position, not estimated.
//!
//! The section-start estimate used to run banking over the code-only text.
//! The banking pass's CALL-exit-bank analysis needs the const-reader
//! regions (their bodies plus the RETLW table data, which follow the
//! code): measured without them, every reader CALL leaves the tracked
//! bank UNKNOWN and banking over-inserts BANKSEL pairs, over-estimating
//! the code end by a few words. When the real end lands in the top of a
//! 256-byte window (the over-estimate wraps into the next window and
//! looks like a fit), the config table's `.table` base crosses its window
//! and the assembler's window assert panics.
//!
//! Fixture: `const_window_drift.c`. Fifteen small `if (g_idx == K)` blocks
//! each do a const-table read (a `__read_t1` CALL) followed by a banked
//! store, the exact context where the unresolved reader call
//! over-inserts a BANKSEL pair; a sixteenth fat block of fourteen more
//! read+store statements lands the real code end at LOW 0xE8, inside the
//! window's top 24 bytes, with the over-estimate wrapping into the next
//! window. Before the fix the driver panicked at assembly (`const table
//! __epic_config of 69 bytes at base 0x3E8 crosses its 256-byte window`);
//! after the fix it assembles and the sim runs.

use std::collections::HashMap;
use std::process::Command;

fn fixture() -> &'static str {
    "tests/fixtures/const_window_drift.c"
}

/// Rebuild the alloc layout for the fixture so the test can locate the
/// observable globals by name (the same pattern `const_table_e2e.rs` and
/// the hal-pic16 slice e2e use). The fixture's `stdint.h`/`epic-cc.h`
/// come from the driver's materialised header dir, exactly like main.rs.
fn fixture_layout() -> HashMap<String, u16> {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let tmp = std::env::temp_dir().join(format!("cwdrift-{}", std::process::id()));
    let header_dir = tmp.join("include");
    std::fs::create_dir_all(&header_dir).expect("create header dir");
    std::fs::write(header_dir.join("epic-cc.h"), driver::epic_cc_h::EPIC_CC_H)
        .expect("write epic-cc.h");
    std::fs::write(header_dir.join("stdint.h"), driver::stdint_h::STDINT_H)
        .expect("write stdint.h");
    let opts = driver::clang::Options {
        includes: vec!["tests/fixtures".to_string()],
        defines: Vec::new(),
        header_dir: Some(header_dir),
        fosc_hz: None,
        packed_structs: false,
    };
    let ll_text =
        driver::clang::compile_to_stdout(&clang, &resdir, std::path::Path::new(fixture()), &opts);
    let _ = std::fs::remove_dir_all(&tmp);
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    layout.globals.clone()
}

/// Compile the fixture with the real `epic-cc` binary and return the
/// parsed program words.
fn compile_fixture() -> Vec<u16> {
    let hex_path =
        std::env::temp_dir().join(format!("const_window_drift_{}.hex", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F877A", "-I", "tests/fixtures"])
        .arg("-o")
        .arg(&hex_path)
        .arg(fixture())
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc failed on the const-window fixture: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read produced hex");
    let _ = std::fs::remove_file(&hex_path);
    pic14_sim::parse_hex(&produced)
}

#[test]
fn config_table_near_window_top_assembles_and_runs() {
    let prog = compile_fixture();
    let globals = fixture_layout();
    let g_idx = *globals.get("g_idx").expect("g_idx global") as usize;
    let g_out = *globals.get("g_out").expect("g_out global") as usize;

    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[g_idx] = 0; // small blocks: only block 0 runs
    p.run(200_000);
    // Block 0: g_out = t1[(0 + 0) & 7] + 0 = t1[0] = 1.
    assert_eq!(p.ram()[g_out], 1, "block 0 store");

    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[g_idx] = 15; // the fat block runs
    p.run(200_000);
    // Fat block's last statement: g_out = t1[(15 + 13) & 7] + 13 = t1[4] + 13.
    assert_eq!(p.ram()[g_out], 5 + 13, "fat block last store");
}
