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

use driver::clang;
use driver::clang_discovery;
use driver::cli;

use clang_discovery::{resolve_clang, resolve_llvm_link, resolve_opt};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

fn main() {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("epic-cc {}", env!("EPIC_CC_STAMP"));
        return;
    }
    let has_device_flag = argv
        .iter()
        .any(|a| matches!(a.as_str(), "--device" | "--target" | "--mcu" | "-mcu"));
    if !has_device_flag {
        let env_device = std::env::var("PIC8_DEVICE").unwrap_or_else(|_| "p16f877a".to_string());
        argv.push("--target".to_string());
        argv.push(env_device);
    }
    let cli = match cli::parse_args(&argv) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    for input in &cli.inputs {
        let lower = input.to_ascii_lowercase();
        if lower.ends_with(".asm") || lower.ends_with(".s") {
            eprintln!(
                "epic-cc: .asm inputs are not supported in this build; use EPIC_NAKED functions"
            );
            std::process::exit(2);
        }
    }

    let device = device::resolve(&cli.device).unwrap_or_else(|| {
        let available = device::ALL
            .iter()
            .map(|d| d.name)
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "epic-cc: unknown device {} (available: {})",
            cli.device, available
        );
        std::process::exit(1);
    });
    if device.core == device::Core::Pic14e {
        eprintln!(
            "epic-cc: device {} has core pic14e which has no backend yet (need isel-pic14e)",
            device.name
        );
        std::process::exit(1);
    }

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
    let opt_bin = match resolve_opt(&clang) {
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
    let header_dir = tmp.join("include");
    std::fs::create_dir_all(&header_dir).expect("create header dir");
    std::fs::write(header_dir.join("epic-cc.h"), driver::epic_cc_h::EPIC_CC_H)
        .expect("write epic-cc.h");
    std::fs::write(header_dir.join("stdint.h"), driver::stdint_h::STDINT_H)
        .expect("write stdint.h");
    std::fs::write(header_dir.join("stdbool.h"), driver::stdbool_h::STDBOOL_H)
        .expect("write stdbool.h");
    std::fs::write(header_dir.join("stddef.h"), driver::stddef_h::STDDEF_H)
        .expect("write stddef.h");
    std::fs::write(header_dir.join("string.h"), driver::string_h::STRING_H)
        .expect("write string.h");
    std::fs::write(header_dir.join("stdlib.h"), driver::stdlib_h::STDLIB_H)
        .expect("write stdlib.h");
    std::fs::write(header_dir.join("xc.h"), driver::xc_h::XC_H).expect("write xc.h");
    std::fs::write(header_dir.join("stdarg.h"), driver::stdarg_h::STDARG_H)
        .expect("write stdarg.h");
    std::fs::write(header_dir.join("stdio.h"), driver::stdio_h::STDIO_H).expect("write stdio.h");

    let sources: Vec<(String, String)> = cli
        .inputs
        .iter()
        .map(|p| {
            (
                p.clone(),
                std::fs::read_to_string(p).unwrap_or_else(|e| {
                    eprintln!("epic-cc: read {p}: {e}");
                    std::process::exit(1);
                }),
            )
        })
        .collect();
    let prescan_spec = driver::prescan::find_epic_config(&sources);
    let fosc_hz: u64 = match &prescan_spec {
        Some(spec) => driver::fosc::resolve_fosc_hz(device, spec),
        None => driver::fosc::resolve_fosc_hz_from_defaults(device),
    };

    // 1. clang: one invocation per translation unit.
    let clang_opts = clang::Options {
        includes: cli.includes.clone(),
        defines: driver::predef::xc8_predefines(device.core, device.name)
            .into_iter()
            .chain(cli.defines.clone())
            .collect(),
        header_dir: Some(header_dir.clone()),
        fosc_hz: Some(fosc_hz),
        packed_structs: device.core == device::Core::Pic18,
    };
    let mut units = Vec::new();
    let mut dep_paths = Vec::new();
    for (n, input) in cli.inputs.iter().enumerate() {
        let ll_path = tmp.join(format!("{n:03}.ll"));
        let dep_path = tmp.join(format!("{n:03}.d"));
        let mut cmd = clang::base_cmd(&clang, &resdir);
        clang::apply_options(&mut cmd, &clang_opts);
        cmd.args(["-MD", "-MF", dep_path.to_str().unwrap()]);
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
        dep_paths.push(dep_path);
    }

    // See `driver::header_detect` (epic-cc#196) for why this reads clang's
    // `-MD` dependency output rather than grepping the raw source text.
    let dep_texts: Vec<String> = dep_paths
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap_or_default())
        .collect();
    let need_string = dep_texts
        .iter()
        .any(|t| driver::header_detect::dep_file_includes(t, "string.h"));
    let need_stdio = dep_texts
        .iter()
        .any(|t| driver::header_detect::dep_file_includes(t, "stdio.h"));
    if need_string {
        let string_c_path = tmp.join("__epic_string.c");
        std::fs::write(&string_c_path, driver::string_c::STRING_C).expect("write string.c");
        let ll_path = tmp.join("__epic_string.ll");
        let mut cmd = clang::base_cmd(&clang, &resdir);
        clang::apply_options(&mut cmd, &clang_opts);
        cmd.args([
            "-o",
            ll_path.to_str().unwrap(),
            string_c_path.to_str().unwrap(),
        ]);
        if cli.verbose {
            eprintln!("epic-cc: {cmd:?}");
        }
        let out = cmd.output().expect("run clang for string.c");
        if !out.status.success() {
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            std::process::exit(1);
        }
        units.push(ll_path);
    }
    if need_stdio {
        let stdio_c_path = tmp.join("__epic_stdio.c");
        std::fs::write(&stdio_c_path, driver::stdio_c::STDIO_C).expect("write stdio.c");
        let ll_path = tmp.join("__epic_stdio.ll");
        let mut cmd = clang::base_cmd(&clang, &resdir);
        clang::apply_options(&mut cmd, &clang_opts);
        cmd.args([
            "-o",
            ll_path.to_str().unwrap(),
            stdio_c_path.to_str().unwrap(),
        ]);
        if cli.verbose {
            eprintln!("epic-cc: {cmd:?}");
        }
        let out = cmd.output().expect("run clang for stdio.c");
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

    // 2.5. Whole-program cleanup: each TU's clang invocation cannot see the
    // rest of the call graph, so a call site's constant argument survives
    // raw into every TU's own .ll. Now that llvm-link has merged the
    // program into one module, run the curated pass list over it
    // (driver::wholeprog_opt) before anything else reads the IR. See
    // crates/driver/src/wholeprog_opt.rs for the pass list and why it
    // preserves the overlay allocator's frame boundaries.
    let opt_path = tmp.join("merged_opt.ll");
    let merged_ll_text = match driver::wholeprog_opt::run(&opt_bin, &merged_path, &opt_path) {
        Ok(text) => text,
        Err(msg) => {
            eprintln!("epic-cc: whole-program opt: {msg}");
            std::process::exit(1);
        }
    };

    let ll_text = irparse::sanitize_symbols(&merged_ll_text);
    let canonical_spec = ll_text
        .find("section \".epiccfg.")
        .map(|i| &ll_text[i + "section \".epiccfg.".len()..])
        .and_then(|rest| rest.split('"').next())
        .map(str::to_string);

    match (&prescan_spec, &canonical_spec) {
        (Some(p), Some(c)) if p != c => panic!(
            "epic-cc: internal inconsistency, the pre-scan found EPIC_CONFIG({p:?}) but the \
             compiled program's actual config is {c:?}; this is a pre-scanner bug, please report it"
        ),
        (Some(_), None) => panic!(
            "epic-cc: the pre-scan found an EPIC_CONFIG(...) invocation that did not survive \
             into the compiled program (likely behind an #ifdef the pre-scan cannot see); v1 \
             requires an unconditional top-level invocation"
        ),
        _ => {}
    }

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
    if let Some(map_path) = &cli.map {
        std::fs::write(map_path, driver::report::map_text(&device, &layout)).expect("write map");
    }

    // 7. isel: IR -> assembly. Locals are keyed `{func}::{name}` in the
    // map, matching what both backends look up. The keys are cloned: the
    // layout is still needed for the size report after isel runs.
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.iter().map(|(k, &v)| (k.clone(), v)));
    addrs.extend(layout.locals.iter().map(|(k, &v)| (k.clone(), v)));
    let asm = match device.core {
        device::Core::Pic14 => isel::select(device, &m, &addrs),
        device::Core::Pic18 => isel_pic18::select(device, &m, &addrs),
        device::Core::Pic14e => {
            eprintln!(
                "epic-cc: pic14e core not yet implemented for {}",
                device.name
            );
            std::process::exit(1);
        }
    };

    let asm = match device.core {
        device::Core::Pic14 => {
            // 8-9. schedule -> banking -> peephole, PIC14 only: PIC18's
            // encoder emits its own access/BSR bits and has no PCLATH, so
            // neither pass has anything to do for PIC18. schedule
            // (ADR-027, epic-cc#210) runs before banking so it sees
            // isel's raw instruction order before banking turns bank
            // demand into BANKSEL text; phase 1 is an identity transform.
            let asm = schedule::schedule(device, &asm);
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
        device::Core::Pic14e => {
            eprintln!(
                "epic-cc: pic14e core not yet implemented for {}",
                device.name
            );
            std::process::exit(1);
        }
    };

    if cli.emit == cli::Emit::Asm {
        std::fs::write(&cli.output, &asm).expect("write asm");
        return;
    }

    // 10. asm: assembly -> Intel HEX (with config words when present). The
    // program words are captured before config insertion: the PIC14 config
    // word lives past the flash ceiling (0x2007 on the 877A), so the hex
    // vec is resized to include it and its length would overcount flash.
    let fuse_spec = canonical_spec
        .as_deref()
        .map(|s| driver::fosc::fuse_spec(s))
        .unwrap_or_default();
    let config_bytes: Option<Vec<u8>> = if canonical_spec.is_some() {
        Some(device::resolve_config(&device.config, &fuse_spec))
    } else {
        None
    };
    let program_words = asm::assemble_words(device, &asm);
    let hex = match (device.core, &config_bytes) {
        (device::Core::Pic14, Some(cb)) => {
            let mut words = program_words.clone();
            let idx = (device.config.base_byte_addr / 2) as usize;
            if words.len() <= idx {
                words.resize(idx + 1, 0);
            }
            let w = u16::from(cb[0]) | (u16::from(cb[1]) << 8);
            words[idx] = w;
            asm::to_hex(&words)
        }
        (device::Core::Pic18, Some(cb)) => {
            let mut config_words = Vec::new();
            for chunk in cb.chunks(2) {
                let lo = chunk[0] as u16;
                let hi = if chunk.len() > 1 { chunk[1] as u16 } else { 0 };
                config_words.push(lo | (hi << 8));
            }
            asm::to_hex_regions(&[
                (0, &program_words),
                (device.config.base_byte_addr, &config_words),
            ])
        }
        _ => asm::to_hex(&program_words),
    };
    if let Some(cb) = &config_bytes {
        eprintln!("epic-cc: resolved configuration for {}:", device.name);
        for (i, b) in cb.iter().enumerate() {
            eprintln!(
                "  byte 0x{:06X} = 0x{b:02X}",
                device.config.base_byte_addr as usize + i
            );
        }
    }
    eprint!(
        "{}",
        driver::report::render_size(&device, &layout, program_words.len())
    );
    std::fs::write(&cli.output, hex).expect("write hex");
}
