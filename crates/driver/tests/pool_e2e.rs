//! Pooled flash string table (epic-cc#815): with `--const-pool` the
//! address-taken const bytes also emit as one `__const_pool` table,
//! byte-identical to the concatenation of the member per-const tables;
//! without the flag the output is unchanged.

use std::process::Command;

fn asm_for(extra: &[&str]) -> String {
    let asm_path = std::env::temp_dir().join(format!("pool_{}.asm", std::process::id()));
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

/// Table label to its RETLW bytes: from `NAME:` through the consecutive
/// `RETLW 0xNN` lines. Reader entries carry no RETLWs, so they never
/// appear here. Panics on a non-hex RETLW (a ref byte), pinning the
/// test to pure-byte tables.
fn tables(asm: &str) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut cur: Option<(String, Vec<u8>)> = None;
    for raw in asm.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_suffix(':') {
            if let Some(t) = cur.take() {
                out.push(t);
            }
            cur = Some((name.to_string(), Vec::new()));
            continue;
        }
        if let Some(hex) = line.strip_prefix("RETLW 0x") {
            let b = u8::from_str_radix(hex.trim(), 16)
                .unwrap_or_else(|_| panic!("non-hex RETLW in pool test fixture: {line}"));
            if let Some((_, bytes)) = cur.as_mut() {
                bytes.push(b);
            }
            continue;
        }
        if let Some(t) = cur.take() {
            out.push(t);
        }
    }
    if let Some(t) = cur.take() {
        out.push(t);
    }
    out.into_iter().filter(|(_, b)| !b.is_empty()).collect()
}

#[test]
fn pool_emits_byte_identical_concatenation() {
    let asm = asm_for(&["--const-pool"]);
    let all = tables(&asm);
    let (pool, per_const): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|(n, _)| n.starts_with("__const_pool"));
    assert_eq!(pool.len(), 1, "one pool chunk expected:\n{asm}");
    assert_eq!(pool[0].0, "__const_pool");
    // Five tabled consts: three literals plus two same-byte arrays.
    // A clang merge change alters this count loudly instead of
    // silently changing what the pool must contain.
    assert_eq!(per_const.len(), 5, "five member tables expected:\n{asm}");
    // Expected pool: members in name order, byte-identical members
    // (dup1/dup2) kept once. Re-expressed from the listing's own
    // bytes, so this pins content and order, not the constructor.
    let mut names: Vec<&String> = per_const.iter().map(|(n, _)| n).collect();
    names.sort();
    let mut expected: Vec<u8> = Vec::new();
    let mut seen: Vec<&Vec<u8>> = Vec::new();
    for name in names {
        let bytes = &per_const.iter().find(|(n, _)| n == name).unwrap().1;
        if seen.iter().any(|b| *b == bytes) {
            continue;
        }
        seen.push(bytes);
        expected.extend_from_slice(bytes);
    }
    assert_eq!(
        pool[0].1, expected,
        "pool must concatenate the deduped member tables"
    );
    // The duplicate's bytes ride once despite two member tables.
    let dup_len = per_const.iter().find(|(n, _)| n == "dup1").unwrap().1.len();
    let total: usize = per_const.iter().map(|(_, b)| b.len()).sum();
    assert_eq!(pool[0].1.len(), total - dup_len);
    // The pool reader is emitted for epic-cc#816 to call.
    assert!(
        asm.lines().any(|l| l.trim() == "__read___const_pool:"),
        "pool reader entry expected:\n{asm}"
    );
}

#[test]
fn pool_is_additive_unflagged_output_unchanged() {
    // Strip every pool trace (chunk spans, reader entries, name
    // references) from the flagged listing: the remainder must equal
    // the unflagged listing byte for byte.
    let flagged = asm_for(&["--const-pool"]);
    let plain = asm_for(&[]);
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
    assert_eq!(
        stripped, plain_lines,
        "flagged output must be plain output plus the pool"
    );
}
