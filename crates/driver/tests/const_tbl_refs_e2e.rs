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

fn layout_for(fixture: &str) -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new(fixture),
        &driver::clang::Options::default(),
    );
    let m = irparse::parse_ll(&ll_text);
    let mut m = wholeprog::merge(m);
    // Mirror main.rs exactly: the pass drops the forwarded load slot, which
    // moves the frame overlay, so addresses must come from post-pass IR.
    driver::const_runs::run(&mut m, device::Core::Pic18);
    let m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    alloc::allocate(&device::PIC18F4550, &m, &callgraph::edges_text(&cg))
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
    let layout = layout_for(fixture);
    let hex_path = std::env::temp_dir().join(format!(
        "const_tbl_refs_{}_{}.hex",
        std::process::id(),
        fixture.replace('/', "_")
    ));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "-o"])
        .arg(&hex_path)
        .arg(fixture)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let _ = std::fs::remove_file(&hex_path);
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(400_000);
    assert!(p.halted(), "{fixture} must halt");
    (p.ram().to_vec(), layout.globals.clone())
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
