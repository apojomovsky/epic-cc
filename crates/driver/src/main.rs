//! End-to-end driver: C source -> clang (.ll) -> IR pipeline -> Intel HEX.
//!
//! Chains every milestone-1 stage crate: `irparse` -> `wholeprog` ->
//! `legalize` -> `callgraph` (depth check vs the device's stack) -> `alloc`
//! (+ address map) -> `isel` -> `banking` -> `peephole` -> `asm`. From `isel`
//! onward, the pipeline branches on `device.core`: PIC14 runs
//! `isel` -> `banking` -> `peephole` -> page-fit verification -> `asm`;
//! PIC18 runs `isel-pic18` -> `asm` directly (no banking/peephole/paging).

mod clang_discovery;

use clang_discovery::resolve_clang;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let c_file = &args[1];
    let hex_out = args.get(2).map(String::as_str).unwrap_or("out.hex");
    let device = &device::PIC16F877A;

    // 1. clang: .c -> .ll (text on stdout). Resolved from the env vars, or
    // from the bundled clang/ directory next to the executable, or a clean
    // error (see clang_discovery).
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let (clang, resdir) = match resolve_clang(&std::env::vars().collect(), &exe_dir) {
        Ok(pair) => pair,
        Err(msg) => {
            eprintln!("epic-cc: {msg}");
            std::process::exit(1);
        }
    };
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
            resdir.to_str().unwrap(),
            "-o",
            "-",
            c_file,
        ])
        .output()
        .expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();

    // 2-5. irparse -> wholeprog -> legalize -> callgraph (depth check vs the
    // device's hardware stack)
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, device.stack_depth as usize);

    // 6. alloc: complete overlay address map (globals + locals per function)
    let layout = alloc::allocate(device, &m, &callgraph::edges_text(&cg));

    // 7. isel: IR -> assembly. Locals are keyed `{func}::{name}` in the
    // map, matching what both backends look up.
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals);
    addrs.extend(layout.locals);
    let asm = match device.core {
        device::Core::Pic14 => isel::select(device, &m, &addrs),
        device::Core::Pic18 => isel_pic18::select(device, &m, &addrs),
    };

    let hex = match device.core {
        device::Core::Pic14 => {
            // 8-9. banking -> peephole (PIC14 only — PIC18's encoder
            // already emits its own access/BSR bits and needs no
            // BANKSEL-equivalent post-pass; no PCLATH exists to elide
            // either, so peephole has nothing to do for PIC18).
            let asm = banking::assign_banks(device, &asm);
            let asm = peephole::optimize(&asm);

            // Issue #17: the page assignment ran on pre-banking sizes; the
            // banking pass inserts BANKSEL words that grow the text. Verify
            // the FINAL layout's page fit — a function that grew across a
            // page boundary has no `.org` anchor, so the assembler's
            // backward-.org panic would never fire and it would silently
            // straddle (label in the lower page, tail in the upper page,
            // intra-function GOTOs misbranching). Panic loudly instead,
            // before assembling. PIC14-specific — no paging on PIC18: a
            // 20-bit GOTO/CALL reaches the whole 32KB flash.
            isel::verify_page_fit(&m, &asm);

            // 10. asm: PIC14 assembly -> Intel HEX
            asm::assemble_file_to_hex(device, &asm)
        }
        device::Core::Pic18 => asm::assemble_file_to_hex(device, &asm),
    };
    std::fs::write(hex_out, hex).expect("write hex");
}
