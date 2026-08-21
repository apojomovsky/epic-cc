//! End-to-end driver: C source -> clang (.ll) -> IR pipeline -> Intel HEX.
//!
//! Chains every milestone-1 stage crate: `irparse` -> `wholeprog` ->
//! `legalize` -> `callgraph` (depth check vs the device's stack) -> `alloc`
//! (+ address map) -> `isel` -> `banking` -> `peephole` -> `asm`. From `isel`
//! onward, the pipeline branches on `device.core`: PIC14 runs
//! `isel` -> `banking` -> `peephole` -> page-fit verification -> `asm`;
//! PIC18 runs `isel-pic18` -> `asm` directly (no banking/peephole/paging).
//!
//! Multiple `.c` inputs are each run through clang separately, then merged
//! with `llvm-link` before `irparse` ever sees them (docs/31 D-7): the
//! merge, not this driver, resolves cross-unit symbols and renames
//! collisions, so `wholeprog` onward sees exactly the single-module shape it
//! always has.

use driver::clang_discovery;
use driver::cli;

use clang_discovery::{resolve_clang, resolve_llvm_link};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cli = match cli::parse_args(&argv) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    let device = match cli.device.as_str() {
        "p16f877a" => &device::PIC16F877A,
        "p18f4550" => &device::PIC18F4550,
        other => {
            eprintln!("epic-cc: unknown device {other} (expected p16f877a or p18f4550)");
            std::process::exit(2);
        }
    };

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
    let llvm_link = match resolve_llvm_link(&clang) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("epic-cc: {msg}");
            std::process::exit(1);
        }
    };

    // Temp directory for the per-unit .ll files and the merged one. With
    // --save-temps these become durable artifacts the user can diff.
    let tmp = match &cli.save_temps {
        Some(d) => std::path::PathBuf::from(d),
        None => std::env::temp_dir().join(format!("epic-cc-{}", std::process::id())),
    };
    std::fs::create_dir_all(&tmp).expect("create temp dir");

    // 1. clang: one invocation per translation unit.
    let mut units = Vec::new();
    for (n, input) in cli.inputs.iter().enumerate() {
        let ll_path = tmp.join(format!("{n:03}.ll"));
        let mut cmd = Command::new(&clang);
        cmd.args([
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-resource-dir",
            resdir.to_str().unwrap(),
        ]);
        for inc in &cli.includes {
            cmd.args(["-I", inc]);
        }
        for def in &cli.defines {
            cmd.args(["-D", def]);
        }
        cmd.args(["-o", ll_path.to_str().unwrap(), input]);
        if cli.verbose {
            eprintln!("epic-cc: {cmd:?}");
        }
        let out = cmd.output().expect("run clang");
        if !out.status.success() {
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            std::process::exit(1);
        }
        units.push(ll_path);
    }

    // 2. llvm-link: N .ll -> one .ll. Merge order is command-line order, so
    // the renaming of colliding internal symbols is deterministic. Running it
    // for a single unit too keeps one code path; it only rewrites the module
    // header and metadata ordering, which irparse already ignores.
    let merged_path = tmp.join("merged.ll");
    let mut cmd = Command::new(&llvm_link);
    cmd.arg("-S");
    for u in &units {
        cmd.arg(u);
    }
    cmd.args(["-o", merged_path.to_str().unwrap()]);
    if cli.verbose {
        eprintln!("epic-cc: {cmd:?}");
    }
    let out = cmd.output().expect("run llvm-link");
    if !out.status.success() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        std::process::exit(1);
    }

    let ll_text =
        irparse::sanitize_symbols(&std::fs::read_to_string(&merged_path).expect("read merged .ll"));
    if cli.emit == cli::Emit::Ll {
        std::fs::write(&cli.output, &ll_text).expect("write .ll");
        return;
    }

    // 3-5. irparse -> wholeprog -> legalize -> callgraph (depth check vs the
    // device's hardware stack)
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    if cli.emit == cli::Emit::Ir {
        std::fs::write(&cli.output, ir::serialize(&m)).expect("write ir");
        return;
    }
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

    let asm = match device.core {
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
            asm
        }
        device::Core::Pic18 => asm,
    };

    if cli.emit == cli::Emit::Asm {
        std::fs::write(&cli.output, &asm).expect("write asm");
        return;
    }

    // 10. asm: assembly -> Intel HEX
    let hex = asm::assemble_file_to_hex(device, &asm);
    std::fs::write(&cli.output, hex).expect("write hex");
}
