//! End-to-end driver: C source -> clang (.ll) -> IR pipeline -> Intel HEX.
//!
//! Chains every milestone-1 stage crate: `irparse` -> `wholeprog` ->
//! `legalize` -> `callgraph` (depth check vs 8) -> `alloc` (+ address map) ->
//! `isel` -> `banking` -> `peephole` -> `asm`.

use std::collections::HashMap;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let c_file = &args[1];
    let hex_out = args.get(2).map(String::as_str).unwrap_or("out.hex");

    // 1. clang: .c -> .ll (text on stdout)
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let ll = Command::new(clang)
        .args([
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-resource-dir",
            &resdir,
            "-o",
            "-",
            c_file,
        ])
        .output()
        .expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();

    // 2-5. irparse -> wholeprog -> legalize -> callgraph (depth check vs 8)
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, 8);

    // 6. alloc: complete overlay address map (globals + locals per function)
    let layout = alloc::allocate(&m, &callgraph::edges_text(&cg));

    // 7. isel: IR -> PIC14 assembly. Locals are keyed `{func}::{name}` in
    // the map, matching what isel looks up for every value.
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals);
    addrs.extend(layout.locals);
    let asm = isel::select(&m, &addrs);

    // 8-9. banking -> peephole
    let asm = banking::assign_banks(&asm);
    let asm = peephole::optimize(&asm);

    // 10. asm: PIC14 assembly -> Intel HEX
    let hex = asm::assemble_file_to_hex(&asm);
    std::fs::write(hex_out, hex).expect("write hex");
}
