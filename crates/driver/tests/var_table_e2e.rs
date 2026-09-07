//! Phase-2 debugger variable table: `--var-table` joins the full `-g`
//! metadata (`DILocalVariable`/`DIType`/`#dbg_*` records) against the
//! allocation map. Acceptance: every covered global type renders with
//! its allocated address, an address-taken local resolves through the
//! SSA-key bridge, and the phase-1 line table is unchanged by the `-g`
//! switch on this fixture.

use std::process::Command;

/// Compile `src` with the driver and return the artifact `flag` wrote.
fn artifact(src: &str, flag: &str, out_path: &str) -> String {
    let status = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            src,
            "-o",
            "/tmp/vt.hex",
            "--device",
            "p16f877a",
            flag,
            out_path,
        ])
        .output()
        .expect("run driver");
    assert!(
        status.status.success(),
        "driver ({flag}): {}",
        String::from_utf8_lossy(&status.stderr)
    );
    std::fs::read_to_string(out_path).unwrap()
}

/// The `(address, TYPE)` of the `global|local <key> 0xNN TYPE` record.
/// TYPE may contain spaces (`struct P`): everything after the address.
fn record(table: &str, key: &str) -> (u16, String) {
    for l in table.lines() {
        let mut it = l.split_whitespace();
        let name = it.nth(1).unwrap_or_default();
        let addr = it.next().unwrap_or_default();
        if name == key && addr.starts_with("0x") {
            let ty: Vec<&str> = it.collect();
            return (u16::from_str_radix(&addr[2..], 16).unwrap(), ty.join(" "));
        }
    }
    panic!("no record for {key} in:\n{table}");
}

#[test]
fn globals_resolve_with_flat_types() {
    let t = artifact(
        "tests/fixtures/var_table.c",
        "--var-table",
        "/tmp/vt_globals.txt",
    );
    assert_eq!(record(&t, "g_int").1, "int");
    assert_eq!(record(&t, "g_char").1, "char");
    assert_eq!(record(&t, "g_long").1, "long");
    assert_eq!(record(&t, "g_p").1, "struct P");
    // Distinct globals must occupy distinct addresses.
    let mut addrs: Vec<u16> = ["g_int", "g_char", "g_long", "g_p"]
        .iter()
        .map(|k| record(&t, k).0)
        .collect();
    addrs.sort_unstable();
    addrs.dedup();
    assert_eq!(addrs.len(), 4, "globals must not overlap: {t}");
}

#[test]
fn address_taken_local_joins_through_ssa_key() {
    let t = artifact(
        "tests/fixtures/var_table.c",
        "--var-table",
        "/tmp/vt_local.txt",
    );
    // `arr` has its address taken, so it survives -O1 in RAM; the join
    // must land it in the overlay locals through its #dbg_assign record.
    let slot = record(&t, "main::arr");
    assert_eq!(slot.1, "int[4]");
    // Plain locals (`slot`, `p`) were promoted and constant-folded at
    // -O1: they must be omitted, not invented.
    assert!(
        !t.contains(" main::slot "),
        "optimized-away local must be omitted: {t}"
    );
}

#[test]
fn var_table_addresses_match_the_map() {
    let map = artifact("tests/fixtures/var_table.c", "--map", "/tmp/vt_map.txt");
    let t = artifact(
        "tests/fixtures/var_table.c",
        "--var-table",
        "/tmp/vt_join.txt",
    );
    // Every global record's address equals the map's for the same name.
    for name in ["g_int", "g_char", "g_long", "g_p"] {
        let map_line = map
            .lines()
            .find(|l| l.starts_with(&format!("global {name} ")))
            .unwrap_or_else(|| panic!("map missing {name}:\n{map}"));
        let map_addr: u16 =
            u16::from_str_radix(&map_line.split_whitespace().nth(2).unwrap()[2..], 16).unwrap();
        assert_eq!(record(&t, name).0, map_addr, "{name} address mismatch");
    }
    // The map keys locals by SSA def name, the var table prints the C
    // name. The invariant is the address set: each local record's
    // address must be a local address the map reports.
    let local_addrs: Vec<u16> = map
        .lines()
        .filter(|l| l.starts_with("local main::"))
        .map(|l| u16::from_str_radix(&l.split_whitespace().nth(2).unwrap()[2..], 16).unwrap())
        .collect();
    for l in t.lines().filter(|l| l.starts_with("local main::")) {
        let addr = u16::from_str_radix(&l.split_whitespace().nth(2).unwrap()[2..], 16).unwrap();
        assert!(
            local_addrs.contains(&addr),
            "var table address 0x{addr:02X} not in map:\n{map}"
        );
    }
}
