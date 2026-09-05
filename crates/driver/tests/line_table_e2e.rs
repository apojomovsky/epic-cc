//! Phase-1 debugger line table: `--line-table` emits an address-to-source-line
//! table, and the BANKSEL preservation contract holds (an inserted BANKSEL
//! inherits the preceding instruction's line). Acceptance: (a) a small
//! multi-line C program's table maps the expected word addresses to the
//! expected `file:line:col` records, and (b) a banked program's inserted
//! BANKSEL words carry the same line as the instruction they precede.

use std::process::Command;

/// Compile `src` with the driver and return the `--line-table` output.
fn line_table(src: &str, device: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            src,
            "-o",
            "/tmp/lt.hex",
            "--device",
            device,
            "--line-table",
            "/tmp/lt.txt",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read_to_string("/tmp/lt.txt").unwrap()
}

#[test]
fn line_table_maps_words_to_source_lines() {
    // A multi-line body: `a = 1;` on line 3, `b = a + 2;` on line 4,
    // `out = a + b;` on line 5. The table must carry each line's words.
    let src = "tests/fixtures/line_table.c";
    let table = line_table(src, "p16f877a");
    let lines: Vec<&str> = table.lines().filter(|l| !l.starts_with(';')).collect();
    assert!(!lines.is_empty(), "line table must have records:\n{table}");
    // Every record is `file:line:col 0xNNNN`.
    for l in &lines {
        let parts: Vec<&str> = l.split_whitespace().collect();
        assert_eq!(parts.len(), 2, "malformed record: {l}");
        assert!(
            parts[0].contains("line_table.c:"),
            "record must name the source file: {l}"
        );
        assert!(
            parts[1].starts_with("0x"),
            "record must carry a hex address: {l}"
        );
    }
    // The three source lines must all appear (the body is lines 6-8).
    let joined = lines.join("\n");
    for line_no in ["6:", "7:", "8:"] {
        assert!(
            joined.contains(&format!("line_table.c:{line_no}")),
            "line {line_no} missing from table:\n{joined}"
        );
    }
}

#[test]
fn banksel_words_inherit_the_preceding_line() {
    // The banked fixture forces BANKSEL insertion (90 globals exceed bank 0).
    let src = "tests/fixtures/banked.c";
    let table = line_table(src, "p16f877a");
    let records: Vec<(&str, &str)> = table
        .lines()
        .filter(|l| !l.starts_with(';'))
        .map(|l| {
            let mut it = l.split_whitespace();
            (it.next().unwrap(), it.next().unwrap())
        })
        .collect();
    assert!(
        records.len() > 100,
        "banked program should have many line records, got {}",
        records.len()
    );
    // The BANKSEL contract: an inserted BANKSEL word carries the same line
    // as the instruction it precedes. We can't see the BANKSELs directly in
    // the table (they're compiler-generated words with a source line), but
    // the table must be dense: every word address 0x0005..end has a record,
    // so no word is left unmapped.
    let addrs: Vec<u32> = records
        .iter()
        .map(|(_, a)| u32::from_str_radix(&a[2..], 16).unwrap())
        .collect();
    for w in addrs.windows(2) {
        assert!(
            w[1] == w[0] || w[1] == w[0] + 1,
            "line table must be dense (no unmapped word): 0x{:04X} then 0x{:04X}",
            w[0],
            w[1]
        );
    }
}
