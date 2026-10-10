//! Pooled literals that escape into a call (epic-cc#1026): a pointer passed to
//! a callee that dereferences it must keep its RAM copy under `--const-pool`.
//! Without it the callee reads flash bytes as RAM and the output diverges from
//! the unpooled build. The simulator must return the same globals either way.

use std::process::Command;

/// RAM address of a global, read off the compiler's own `--map` output.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

/// Compiles `fixture` with `extra` flags, simulates it for `budget` cycles,
/// and returns whether it halted plus the named globals' RAM bytes.
fn run(
    fixture: &str,
    tag: &str,
    extra: &[&str],
    globals: &[&str],
    budget: usize,
) -> (bool, Vec<u8>) {
    // Unique temp paths: the suite runs tests in parallel.
    let dir = std::env::temp_dir();
    let stem = format!("pool_escape_{tag}_{}", std::process::id());
    let hex_path = dir.join(format!("{stem}.hex"));
    let map_path = dir.join(format!("{stem}.map"));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .arg(fixture)
        .arg("-o")
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .args(["--device", "p16f877a"])
        .args(extra)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {tag}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).unwrap();
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);

    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
    p.run(budget);
    let ram = p.ram();
    let vals = globals.iter().map(|g| ram[map_addr(&map, g)]).collect();
    (p.halted(), vals)
}

#[test]
fn pooled_pointer_into_noinline_callee_matches_unpooled() {
    let f = "tests/fixtures/pool_call_escape.c";
    let g = ["g_direct", "g_offset"];
    let (halted, base) = run(f, "base", &[], &g, 200_000);
    assert!(halted, "unpooled build must halt");
    assert_eq!(base, [108, 108], "unpooled build: pick('l') on both args");
    let (halted, pooled) = run(f, "pool", &["--const-pool"], &g, 200_000);
    assert!(halted, "--const-pool build must halt");
    assert_eq!(pooled, base, "--const-pool build");
}

#[test]
fn pooled_printf_format_matches_unpooled() {
    // The format string is a pooled literal passed to the printf family; each
    // `putchar` call bumps g_n, so the counts must match the unpooled build.
    let f = "tests/fixtures/stdio_printf_no_varargs.c";
    let (halted, base) = run(f, "stdio_base", &[], &["g_n"], 200_000);
    assert!(halted, "unpooled stdio fixture must halt");
    assert_eq!(base, [3], "unpooled build: three putchar calls");
    let (halted, pooled) = run(f, "stdio_pool", &["--const-pool"], &["g_n"], 200_000);
    assert!(halted, "--const-pool stdio fixture must halt");
    assert_eq!(pooled, base, "--const-pool build");
}

#[test]
fn pooled_strlen_consumer_matches_unpooled() {
    // consume() passes its pointer to strlen, which reads the bytes: the
    // direct-only literals keep their staged RAM copy, so g_sink matches.
    let f = "tests/fixtures/pool_addrs_mixed.c";
    let (halted, base) = run(f, "strlen_base", &[], &["g_sink"], 200_000);
    assert!(halted, "unpooled build must halt");
    assert_eq!(base, [19], "strlen of mixed[0..] payload");
    let (halted, pooled) = run(f, "strlen_pool", &["--const-pool"], &["g_sink"], 200_000);
    assert!(halted, "--const-pool build must halt");
    assert_eq!(pooled, base, "--const-pool build");
}
