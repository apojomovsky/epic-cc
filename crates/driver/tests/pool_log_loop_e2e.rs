//! Pool-reader log loop (epic-cc#817): the mdb `epic_harness_log` byte
//! loop over a pool-resident string lowers as `idx = ptr - POOL_BASE`
//! once (here an (in-chunk index, chunk id) pair per call), then the
//! existing table `CALL` per byte with the index advancing alongside.
//! The fixture mixes direct, select, and RAM call sites; `g_sel` stays
//! 0, so the else arms log. Proof is sim execution: the pooled build
//! links with no literal RAM copies and its transcript is byte-identical
//! to the RAM-backed build. The pool spans two chunks with logged
//! shorts in each, so both dispatch paths run.

use std::process::Command;

/// With `g_sel` 0 the selects take their else arms, then `four`, then
/// the 15-byte RAM buffer (`A`..`O`).
fn expected() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"one\ntwo\nthree\nfour\nABCDEFGHIJKLMNO");
    v
}

fn build(tag: &str, extra: &[&str]) -> (String, String, String) {
    // Unique paths per test: the suite runs tests in parallel and two
    // tests sharing one temp file delete each other's output.
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let hex = dir.join(format!("pool_log_{tag}_{pid}.hex"));
    let map = dir.join(format!("pool_log_{tag}_{pid}.map"));
    let asm = dir.join(format!("pool_log_{tag}_{pid}.asm"));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/pool_log_loop.c", "-o"])
        .arg(&hex)
        .arg("--map")
        .arg(&map)
        .args(["--device", "p16f877a"])
        .args(extra)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The asm needs its own run: one `--emit` per command.
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/pool_log_loop.c", "-o"])
        .arg(&asm)
        .args(["--device", "p16f877a", "--emit", "asm"])
        .args(extra)
        .output()
        .expect("run driver for asm");
    assert!(
        out.status.success(),
        "driver asm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex).unwrap();
    let map = std::fs::read_to_string(&map).unwrap();
    let asm = std::fs::read_to_string(&asm).unwrap();
    (hex, map, asm)
}
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

/// Run the hex in the simulator, return the `g_tx` prefix of `g_len`.
fn transcript(hex: &str, map: &str) -> Vec<u8> {
    let prog = pic14_sim::parse_hex(hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(200_000);
    assert!(p.halted(), "log loop fixture must halt");
    let tx = map_addr(map, "g_tx");
    let len = map_addr(map, "g_len");
    let n = p.ram()[len] as usize;
    assert!(n <= 80, "transcript length {n} exceeds the buffer");
    p.ram()[tx..tx + n].to_vec()
}

/// RETLW bytes of one asm table label.
fn table_bytes(asm: &str, label: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut in_table = false;
    for line in asm.lines() {
        let t = line.trim();
        if !in_table {
            if t == format!("{label}:") {
                in_table = true;
            }
            continue;
        }
        if let Some(hex) = t.strip_prefix("RETLW 0x") {
            out.push(u8::from_str_radix(hex.trim(), 16).expect("hex RETLW"));
        } else {
            break;
        }
    }
    assert!(!out.is_empty(), "no table bytes under {label}");
    out
}

#[test]
fn pool_log_loop_links_without_copies_and_matches_ram_output() {
    let (hex, map, asm) = build("flagged", &["--const-pool"]);
    assert!(
        !map.lines().any(|l| {
            l.starts_with("global .str") || l.starts_with("staged ") || l.contains("__const_stage")
        }),
        "literals keep no RAM copies and nothing stages: {map}"
    );
    assert!(
        asm.contains("__const_pool_1:"),
        "pool spans two chunks:\n{asm}"
    );
    assert!(
        asm.contains("CALL __epic_log_pool"),
        "pool sites route to the variant"
    );
    assert_eq!(
        asm.matches("CALL epic_harness_log").count(),
        1,
        "only the RAM site keeps the original"
    );
    // A logged short lives past the boundary, so the chunk-1 dispatch
    // path executes rather than only the chunk-0 fallthrough.
    let chunk1 = table_bytes(&asm, "__const_pool_1");
    let has_logged = [b"three".as_slice(), b"four".as_slice()]
        .iter()
        .any(|pat| chunk1.windows(pat.len()).any(|w| w == *pat));
    assert!(has_logged, "a logged short sits in chunk 1");
    assert_eq!(transcript(&hex, &map), expected());
}

#[test]
fn pool_log_loop_ram_backed_baseline_matches() {
    let (hex, map, _) = build("unflagged", &[]);
    assert_eq!(transcript(&hex, &map), expected());
}
