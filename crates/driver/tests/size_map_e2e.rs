//! CC-6 reporting acceptance: the size report on stderr matches the HEX and
//! the allocator's layout, and --map writes the allocator's map text.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("epic-cc-size-map-{}-{name}", std::process::id()));
    p
}

fn fixture_add() -> String {
    format!("{}/tests/fixtures/add.c", env!("CARGO_MANIFEST_DIR"))
}

/// Run the full pipeline on add.c exactly as the driver does and return the
/// alloc layout, so the report can be checked against the allocator's own
/// facts.
fn add_layout() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/add.c"),
        &driver::clang::Options::default(),
    );
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg))
}

#[test]
fn size_report_matches_hex_and_layout() {
    let hex_path = tmp("add.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            &fixture_add(),
            "-o",
            hex_path.to_str().unwrap(),
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Flash: count the program words in the HEX (the highest nonzero word
    // address + 1, the same trim to_hex applies).
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let flash_used = prog
        .iter()
        .rposition(|&w| w != 0)
        .map(|i| i + 1)
        .unwrap_or(0);

    let layout = add_layout();
    let report = String::from_utf8_lossy(&out.stderr);
    assert!(
        report.contains(&format!("flash: {flash_used}/8192 words")),
        "flash line missing or wrong: {report}"
    );
    // RAM: the report's bank lines must match the layout's bank_used and
    // the device's bank sizes.
    for (i, &used) in layout.bank_used.iter().enumerate() {
        let (start, end) = device::PIC16F877A.ram_banks[i];
        let total = end - start + 1;
        assert!(
            report.contains(&format!("bank {i}: {used}/{total} bytes")),
            "bank {i} line missing or wrong: {report}"
        );
    }
    // The report states what it means by used.
    assert!(
        report.contains("overlay"),
        "RAM line must state the overlay definition: {report}"
    );
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn map_file_matches_the_allocator_map() {
    let hex_path = tmp("map.hex");
    let map_path = tmp("add.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            &fixture_add(),
            "-o",
            hex_path.to_str().unwrap(),
            "--device",
            "p16f877a",
            "--map",
            map_path.to_str().unwrap(),
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let layout = add_layout();
    let written = std::fs::read_to_string(&map_path).unwrap();
    assert_eq!(
        written,
        driver::report::map_text(&device::PIC16F877A, &layout)
    );
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
}
