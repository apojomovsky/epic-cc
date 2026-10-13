//! Pooled flash string table (epic-cc#815, default per epic-cc#1211):
//! address-taken const bytes emit as one `__const_pool` table only when
//! a routed site reads through it (epic-cc#1088). The pool_strings
//! fixture stages every member, so its pooled build carries no table;
//! `--no-const-pool` restores the per-const staging output.

use std::process::Command;

fn asm_for(tag: &str, extra: &[&str]) -> String {
    // Unique path per test: the suite runs tests in parallel and two
    // tests sharing one temp file delete each other's output.
    let asm_path = std::env::temp_dir().join(format!("pool_{tag}_{}.asm", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/pool_strings.c", "-o"])
        .arg(&asm_path)
        .args(["--device", "p16f877a", "--emit", "asm"])
        .args(extra)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver pool_strings{}: {}",
        if extra.is_empty() {
            ""
        } else {
            " --const-pool"
        },
        String::from_utf8_lossy(&out.stderr)
    );
    let asm = std::fs::read_to_string(&asm_path).unwrap();
    let _ = std::fs::remove_file(&asm_path);
    asm
}

#[test]
fn pool_without_routed_site_emits_no_table() {
    // epic-cc#1088: every pool_strings member stages (its consumer
    // reads through INDF), so no site reads through the pool. The
    // pooled listing carries no table, reader, or reference, and
    // matches the opt-out listing exactly.
    let flagged = asm_for("no-table-flagged", &["--const-pool"]);
    assert!(
        !flagged.lines().any(|l| l.contains("__const_pool")),
        "no pool table, reader, or reference expected:\n{flagged}"
    );
    let plain = asm_for("no-table-plain", &["--no-const-pool"]);
    assert_eq!(
        flagged, plain,
        "pooled output must equal opt-out output with no routed site"
    );
}

/// Drop every `.align 256` line that directly precedes a `.table` line.
fn drop_table_aligns(lines: Vec<&str>) -> Vec<&str> {
    lines
        .iter()
        .enumerate()
        .filter(|(i, l)| {
            !(l.trim() == ".align 256"
                && lines
                    .get(i + 1)
                    .is_some_and(|n| n.trim().starts_with(".table ")))
        })
        .map(|(_, l)| *l)
        .collect()
}

#[test]
fn pool_is_additive_over_opt_out_output() {
    // Strip every pool trace (chunk spans, reader entries, name
    // references) from the pooled listing: the remainder must equal
    // the opt-out listing byte for byte, modulo table window padding.
    let flagged = asm_for("additive-flagged", &["--const-pool"]);
    let plain = asm_for("additive-plain", &["--no-const-pool"]);
    let mut stripped: Vec<&str> = Vec::new();
    let mut lines = flagged.lines().peekable();
    let mut in_pool_data = false;
    while let Some(raw) = lines.next() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if in_pool_data {
            // The pool RETLW run carries no name: it ends at the next
            // label, which belongs to the plain listing again.
            if line.ends_with(':') {
                in_pool_data = false;
            } else {
                continue;
            }
        }
        if line.starts_with("__read___const_pool") {
            for _ in 0..6 {
                lines.next();
            }
            // A pool chunk's own window align carries no pool name; it
            // sits right after its reader entry, so skip it here rather
            // than failing on innocent layout shifts.
            if lines.peek().is_some_and(|l| l.trim() == ".align 256") {
                lines.next();
            }
            continue;
        }
        if line == "__const_pool:" || line.starts_with("__const_pool_") {
            in_pool_data = true;
            continue;
        }
        if line.contains("__const_pool") {
            continue;
        }
        stripped.push(raw);
    }
    // Drop trailing empty lines the pool section leaves behind so the
    // comparison is about content, not blank padding.
    while stripped.last().is_some_and(|l| l.trim().is_empty()) {
        stripped.pop();
    }
    let mut plain_lines: Vec<&str> = plain.lines().collect();
    while plain_lines.last().is_some_and(|l| l.trim().is_empty()) {
        plain_lines.pop();
    }
    // A table's `.align 256` is placement-driven window padding: the pool
    // table moves code, so either side may carry one where the other does
    // not. Each align is decided from measured bases at emission, and a
    // full build rechecks every window in the assembler.
    let stripped = drop_table_aligns(stripped);
    let plain_lines = drop_table_aligns(plain_lines);
    assert_eq!(
        stripped, plain_lines,
        "flagged output must be plain output plus the pool"
    );
}

/// Value pin for the staged walks (epic-cc#1141): every member stages
/// through the shared buffer, so the consumer loop reads RAM, not flash.
/// The last call walks `dup2` (`"same-bytes"`), so `g_sink` must end as
/// `s`. The shape assertions above cannot catch a misread walk.
#[test]
fn pool_staged_walks_read_correct_bytes() {
    let hex_path = std::env::temp_dir().join(format!("pool_values_{}.hex", std::process::id()));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/pool_strings.c",
            "-o",
            hex_path.to_str().unwrap(),
            "--map",
            map_path.to_str().unwrap(),
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver pool_strings: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    let prefix = "global g_sink 0x";
    let addr = usize::from_str_radix(
        map.lines()
            .find(|l| l.starts_with(prefix))
            .expect("map entry for g_sink")[prefix.len()..]
            .trim(),
        16,
    )
    .expect("map address is hex");
    let mut p = pic14_sim::Pic14::with_device(&device::PIC16F877A, pic14_sim::parse_hex(&hex));
    p.run(2_000_000);
    assert_eq!(p.ram()[addr], 0x73, "last walked byte must be 's'");
    assert!(p.halted());
}
