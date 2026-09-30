//! epic-cc#639 acceptance: constant bytes holding `LOW(sym)`/`HIGH(sym)`
//! of flash targets copy from a flash table through one counted `TBLRD`
//! loop instead of one `MOVLW`/`MOVWF` pair per byte.
//!
//! Two shapes: an 8-byte function-pointer aggregate clang copies as a wide
//! load/store pair (forwarded into a memcpy), and twelve field-by-field
//! constant stores (synthesized into one table plus a memcpy). Both assert
//! the loop shape in `--emit asm` and the landed bytes in the sim: the
//! copied function pointer must actually dispatch.

use std::collections::HashMap;
use std::process::Command;

/// `in` and `out`'s RAM addresses, read off the compiler's own `--map`
/// output. Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts: the PIC18 path alone parses with
/// switches preserved, and once the frames sit below the globals a
/// difference that far upstream moves every global address.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn asm_for(fixture: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "const_tbl_refs_{}_{}.asm",
        std::process::id(),
        fixture.replace('/', "_")
    ));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "--emit", "asm", "-o"])
        .arg(&path)
        .arg(fixture)
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

fn main_body<'a>(asm: &'a str) -> &'a str {
    asm.split("main:")
        .nth(1)
        .and_then(|rest| rest.split("RETURN").next())
        .expect("main body")
}

fn run_hex(fixture: &str) -> (Vec<u8>, HashMap<String, u16>) {
    let hex_path = std::env::temp_dir().join(format!(
        "const_tbl_refs_{}_{}.hex",
        std::process::id(),
        fixture.replace('/', "_")
    ));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg(fixture)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(400_000);
    assert!(p.halted(), "{fixture} must halt");
    let mut globals = HashMap::new();
    globals.insert("called".to_string(), map_addr(&map, "called") as u16);
    globals.insert("sink".to_string(), map_addr(&map, "sink") as u16);
    (p.ram().to_vec(), globals)
}

#[test]
fn forwarded_const_aggregate_copies_by_loop_and_dispatches() {
    let fixture = "tests/fixtures/const_tbl_refs_fwd.c";
    let asm = asm_for(fixture);
    let body = main_body(&asm);
    assert_eq!(
        body.matches("TBLRD").count(),
        1,
        "the 8-byte copy must run as one loop, not 8 unrolled reads:\n{asm}"
    );
    for f in ["fa", "fb", "fc", "fd"] {
        assert!(
            !body.contains(&format!("LOW({f})")),
            "no inline pointer materialization for {f}:\n{asm}"
        );
    }
    let (ram, globals) = run_hex(fixture);
    let called = globals["called"] as usize;
    let sink = globals["sink"] as usize;
    assert_eq!(ram[called], 0xA1, "the copied f0 pointer must dispatch");
    assert_eq!(ram[sink], 0xA1, "probe returns through the copied struct");
}

#[test]
fn constant_store_run_copies_by_loop_and_dispatches() {
    let fixture = "tests/fixtures/const_tbl_refs_stores.c";
    let asm = asm_for(fixture);
    let body = main_body(&asm);
    assert_eq!(
        body.matches("TBLRD").count(),
        1,
        "the 12-byte run must run as one loop:\n{asm}"
    );
    assert!(
        asm.contains("__tbl.init.main.0:"),
        "the run must synthesize one flash table:\n{asm}"
    );
    for f in ["fa", "fb", "fc"] {
        assert!(
            !body.contains(&format!("LOW({f})")),
            "no inline pointer materialization for {f}:\n{asm}"
        );
    }
    let (ram, globals) = run_hex(fixture);
    let called = globals["called"] as usize;
    let sink = globals["sink"] as usize;
    assert_eq!(ram[called], 0xA1, "the copied f0 pointer must dispatch");
    assert_eq!(
        ram[sink],
        0x7E ^ 0x01 ^ 0x05,
        "tag and extra bytes must land intact"
    );
}
