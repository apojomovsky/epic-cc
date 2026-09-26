//! End-to-end driver: C source -> clang (.ll) -> IR pipeline -> Intel HEX.
//!
//! Chains every stage crate: `irparse` -> `wholeprog` ->
//! `legalize` -> `callgraph` (depth check vs the device's stack) -> `alloc`
//! (+ address map) -> `isel` -> `banking` -> `peephole` -> `asm`. From `isel`
//! onward, the pipeline branches on `device.core`: PIC14 and PIC14E run
//! `isel`/`isel-pic14e` -> `schedule` -> `banking` -> `peephole` ->
//! page-fit verification -> `asm` (banking emits `MOVLB`/`BSR` on PIC14E,
//! RP-bit `BANKSEL` on classic PIC14); PIC18 runs `isel-pic18` ->
//! `outline` (code factoring, `--no-outline` skips it) -> `asm` (no
//! banking/peephole/paging).
//!
//! Multiple `.c` inputs are each run through clang separately, then merged
//! with `llvm-link` before `irparse` ever sees them (docs/31 §7): the
//! merge, not this driver, resolves cross-unit symbols and renames
//! collisions, so `wholeprog` onward sees exactly the single-module shape it
//! always has.

use clang_discovery::{resolve_clang, resolve_llvm_link, resolve_opt};
use driver::clang;
use driver::clang_discovery;
use driver::cli;
use driver::diag;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Resolve `name` via `device::resolve()` or print the same "unknown
/// device" diagnostic and exit 1, whichever call site is asking: the
/// normal compile path and `--resolve-device` (main.rs) both need it.
fn resolve_or_exit(name: &str) -> &'static device::Device {
    device::resolve(name).unwrap_or_else(|| {
        let available = device::ALL
            .iter()
            .map(|d| d.name)
            .collect::<Vec<_>>()
            .join(", ");
        diag::error(&format!("unknown device {name} (available: {available})"))
    })
}

/// The effective header dir: the `include/` shipped beside the executable
/// when present, else a materialized per-run fallback (docs/46 D-6). The
/// fallback parent is a per-pid temp dir, the same shape the compile path
/// uses for its stage artifacts.
fn include_dir_path() -> std::path::PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let fallback_parent = std::env::temp_dir().join(format!("epic-cc-{}", std::process::id()));
    match driver::include_dir::resolve(&exe_dir, &fallback_parent) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("epic-cc: materialize headers: {e}");
            std::process::exit(1);
        }
    }
}
fn main() {
    diag::install_panic_hook();
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("epic-cc {}", env!("EPIC_CC_STAMP"));
        return;
    }
    if let Some(pos) = argv.iter().position(|a| a == "--resolve-device") {
        let name = argv.get(pos + 1).unwrap_or_else(|| {
            eprintln!("epic-cc: error: --resolve-device needs a value");
            std::process::exit(2);
        });
        println!("{}", resolve_or_exit(name).name);
        return;
    }
    if argv.iter().any(|a| a == "--print-include-dir") {
        println!("{}", include_dir_path().display());
        return;
    }
    if let Some(pos) = argv.iter().position(|a| a == "--dump-include-dir") {
        let dir = argv.get(pos + 1).unwrap_or_else(|| {
            eprintln!("epic-cc: --dump-include-dir needs a value");
            std::process::exit(2);
        });
        let dir = std::path::PathBuf::from(dir);
        match driver::include_dir::materialize(&dir) {
            Ok(()) => println!("{}", dir.display()),
            Err(e) => {
                eprintln!("epic-cc: write {}: {e}", dir.display());
                std::process::exit(1);
            }
        }
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
                "epic-cc: error: .asm inputs are not supported in this build; use EPIC_NAKED functions"
            );
            std::process::exit(2);
        }
    }

    let device = resolve_or_exit(&cli.device);

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let (clang, resdir) = match resolve_clang(&std::env::vars().collect(), &exe_dir) {
        Ok(pair) => pair,
        Err(msg) => diag::error(&msg),
    };
    let llvm_link = match resolve_llvm_link(&clang) {
        Ok(p) => p,
        Err(msg) => diag::error(&msg),
    };
    let opt_bin = match resolve_opt(&clang) {
        Ok(p) => p,
        Err(msg) => diag::error(&msg),
    };

    // Temp directory for the per-unit .ll files and the merged one. With
    // --save-temps these become durable artifacts the user can diff.
    let tmp = match &cli.save_temps {
        Some(d) => std::path::PathBuf::from(d),
        None => std::env::temp_dir().join(format!("epic-cc-{}", std::process::id())),
    };
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    // Shipped `include/` beside the binary when present (docs/46 D-6);
    // otherwise the same headers materialized under the temp dir.
    let header_dir = match driver::include_dir::bundled(&exe_dir) {
        Some(dir) => dir,
        None => {
            let dir = tmp.join("include");
            driver::include_dir::materialize(&dir).expect("write headers");
            dir
        }
    };

    let sources: Vec<(String, String)> = cli
        .inputs
        .iter()
        .map(|p| {
            (
                p.clone(),
                std::fs::read_to_string(p)
                    .unwrap_or_else(|e| diag::error(&format!("read {p}: {e}"))),
            )
        })
        .collect();
    // Config comes from exactly one spelling: `EPIC_CONFIG("...")` or
    // `#pragma config` lines. clang drops unknown pragmas before the
    // `.ll`, so the driver recovers them from the raw sources here and
    // lowers them through the same resolution below.
    let epics = driver::prescan::find_epic_configs(&sources);
    let pragmas = driver::prescan::find_pragma_config(&sources);
    let epic = driver::prescan::single_epic(epics);
    if epic.is_some() && !pragmas.is_empty() {
        let e = epic.as_ref().expect("checked above");
        let p = &pragmas[0];
        diag::error_at(
            &p.file,
            p.line,
            p.col,
            &format!(
                "#pragma config cannot be mixed with EPIC_CONFIG in one program \
                 (EPIC_CONFIG at {}:{}:{}); use one spelling",
                e.file, e.line, e.col,
            ),
        );
    }
    let pragma_spec: Option<String> = if pragmas.is_empty() {
        None
    } else {
        Some(driver::prescan::pragma_spec(&device.config, &pragmas))
    };
    // The second element of each arm is unreachable: mixing already
    // errored above, so an epic hit means no pragma hit.
    let (prescan_spec, prescan_loc): (Option<String>, Option<(String, u32, u32)>) =
        match (epic, &pragma_spec) {
            (Some(e), _) => (Some(e.spec), Some((e.file, e.line, e.col))),
            (None, Some(s)) => {
                let p = &pragmas[0];
                (Some(s.clone()), Some((p.file.clone(), p.line, p.col)))
            }
            (None, None) => (None, None),
        };
    let fosc_hz: u64 = match (&prescan_spec, &prescan_loc) {
        (Some(spec), Some((file, line, col))) => driver::fosc::try_resolve_fosc_hz(device, spec)
            .unwrap_or_else(|e| diag::error_at(file, *line, *col, &e)),
        (Some(_), None) => unreachable!("a prescan spec always carries its site"),
        (None, _) => driver::fosc::resolve_fosc_hz_from_defaults(device),
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
    let need_stdlib = dep_texts.iter().any(|t| {
        driver::header_detect::dep_file_includes(t, "stdlib.h")
            || driver::header_detect::dep_file_includes(t, "malloc.h")
    });
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
    if need_stdlib {
        let stdlib_c_path = tmp.join("__epic_stdlib.c");
        std::fs::write(&stdlib_c_path, driver::stdlib_c::STDLIB_C).expect("write stdlib.c");
        let ll_path = tmp.join("__epic_stdlib.ll");
        let mut cmd = clang::base_cmd(&clang, &resdir);
        clang::apply_options(&mut cmd, &clang_opts);
        cmd.args([
            "-o",
            ll_path.to_str().unwrap(),
            stdlib_c_path.to_str().unwrap(),
        ]);
        if cli.verbose {
            eprintln!("epic-cc: {cmd:?}");
        }
        let out = cmd.output().expect("run clang for stdlib.c");
        if !out.status.success() {
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            std::process::exit(1);
        }
        units.push(ll_path);
    }

    let need_math = dep_texts
        .iter()
        .any(|t| driver::header_detect::dep_file_includes(t, "math.h"));
    if need_math {
        let math_c_path = tmp.join("__epic_math.c");
        std::fs::write(&math_c_path, driver::math_c::MATH_C).expect("write math.c");
        let ll_path = tmp.join("__epic_math.ll");
        let mut cmd = clang::base_cmd(&clang, &resdir);
        clang::apply_options(&mut cmd, &clang_opts);
        cmd.args([
            "-o",
            ll_path.to_str().unwrap(),
            math_c_path.to_str().unwrap(),
        ]);
        if cli.verbose {
            eprintln!("epic-cc: {cmd:?}");
        }
        let out = cmd.output().expect("run clang for math.c");
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
    let merged_ll_text =
        match driver::wholeprog_opt::run(&opt_bin, &merged_path, &opt_path, device.core) {
            Ok(text) => text,
            Err(msg) => diag::error(&format!("whole-program opt: {msg}")),
        };

    let ll_text = irparse::sanitize_symbols(&merged_ll_text);
    let canonical_spec = ll_text
        .find("section \".epiccfg.")
        .map(|i| &ll_text[i + "section \".epiccfg.".len()..])
        .and_then(|rest| rest.split('"').next())
        .map(str::to_string);
    match (&prescan_spec, &pragma_spec, &prescan_loc, &canonical_spec) {
        // All three means the macro hid in a header the pre-scan never
        // reads while the sources carry `#pragma config`: `EPIC_CONFIG`
        // text beside pragma text already errored before clang, so this
        // arm is exactly the header-hidden mixing case. It sorts first
        // because the joined pragma spec never equals the compiled one.
        (_, Some(_), Some((file, line, col)), Some(_)) => diag::error_at(
            file,
            *line,
            *col,
            "the compiled program contains an EPIC_CONFIG section but the \
             sources use #pragma config; mixing the two spellings in one program is an error",
        ),
        (Some(p), None, _, Some(c)) if p != c => panic!(
            "internal inconsistency, the pre-scan found EPIC_CONFIG({p:?}) but the \
             compiled program's actual config is {c:?}; this is a pre-scanner bug"
        ),
        (Some(_), None, Some((file, line, col)), None) => diag::error_at(
            file,
            *line,
            *col,
            "the EPIC_CONFIG(...) invocation did not survive \
             into the compiled program (likely behind an #ifdef the pre-scan cannot see); v1 \
             requires an unconditional top-level invocation",
        ),
        _ => {}
    }

    if cli.emit == cli::Emit::Ll {
        std::fs::write(&cli.output, &ll_text).expect("write .ll");
        return;
    }

    // 3-5. irparse -> wholeprog -> legalize -> callgraph (depth check vs the
    // device's hardware stack)
    // Only the PIC18 backend tables dense switches; every other core keeps
    // irparse's compare-chain expansion (irparse is target-agnostic, the
    // driver is where the core is known).
    let preserve_switches = device.core == device::Core::Pic18;
    let mut m = irparse::parse_ll_opts(&ll_text, preserve_switches);
    m = wholeprog::merge(m);
    if cli.emit == cli::Emit::Ir {
        std::fs::write(&cli.output, ir::serialize(&m)).expect("write ir");
        return;
    }
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    // A too-deep call chain is the program's fault (recursion or nesting
    // past the silicon stack), so it reports as an error, not an ICE.
    if cg.max_depth > device.stack_depth as usize {
        diag::error(&format!(
            "callgraph: depth {} exceeds hardware stack {} (recursion is rejected on this device)",
            cg.max_depth, device.stack_depth
        ));
    }

    // 6. alloc: complete overlay address map (globals + locals per function)
    let layout = alloc::allocate(device, &m, &callgraph::edges_text(&cg));
    if let Some(map_path) = &cli.map {
        std::fs::write(map_path, driver::report::map_text(&device, &layout)).expect("write map");
    }
    if let Some(vt_path) = &cli.var_table {
        let dbg_vars = irparse::parse_debug_vars(&ll_text);
        std::fs::write(
            vt_path,
            driver::report::var_table_text(&device, &layout, &dbg_vars),
        )
        .expect("write var table");
    }

    // 7. isel: IR -> assembly. Locals are keyed `{func}::{name}` in the
    // map, matching what both backends look up. The keys are cloned: the
    // layout is still needed for the size report after isel runs.
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.iter().map(|(k, &v)| (k.clone(), v)));
    addrs.extend(layout.locals.iter().map(|(k, &v)| (k.clone(), v)));
    let (asm, mut locs) = match device.core {
        device::Core::Pic14 => isel::select_with_locs(device, &m, &addrs),
        device::Core::Pic18 => isel_pic18::select_with_locs(
            device,
            &m,
            &addrs,
            layout.isr_low_save,
            layout.isr_save,
            layout.isr_hi_save,
        ),
        device::Core::Pic14e => isel_pic14e::select_with_locs(device, &m, &addrs),
        device::Core::PicBaseline => isel_pic_baseline::select_with_locs(device, &m, &addrs),
    };

    let asm = match device.core {
        device::Core::Pic14 | device::Core::Pic14e => {
            // 8-9. schedule -> banking -> peephole, PIC14 and PIC14E only:
            // PIC18's encoder emits its own access/BSR bits and has no
            // PCLATH, so neither pass has anything to do for PIC18. schedule
            // (ADR-027, epic-cc#210) runs before banking so it sees
            // isel's raw instruction order before banking turns bank
            // demand into BANKSEL/MOVLB text. The banking pass is core-aware
            // (docs/33 §1): it emits the classic RP-bit `BCF/BSF STATUS, 5/6`
            // BANKSEL on PIC14 and the single `MOVLB k` on PIC14E, tracking
            // BSR there.
            let (asm, l) = schedule::schedule_with_locs(device, &asm, &locs);
            locs = l;
            let (asm, l) = banking::assign_banks_with_locs(device, &asm, &locs);
            locs = l;
            let (asm, l) = peephole::optimize_with_locs(&asm, &locs);
            locs = l;

            // The page assignment ran on pre-banking sizes; banking inserts
            // BANKSEL/MOVLB words that grow the text. Verify the FINAL layout's
            // page fit: a function grown across a page boundary has no `.org`
            // anchor, so the assembler's backward-.org panic never fires and it
            // silently straddles (label below, tail above, GOTOs misbranch).
            // Panics here instead, before assembling (epic-cc#17). PIC14/PIC14E
            // page GOTOs via PCLATH; PIC18's 20-bit GOTO/CALL reach all flash.
            if device.core == device::Core::Pic14 {
                isel::verify_page_fit(&m, &asm);
            } else {
                isel_pic14e::verify_page_fit(&m, &asm);
            }
            asm
        }
        // Code factoring shares repeated runs as leaf bodies; its own
        // budget check adds one return level to the IR depth, so it backs
        // off rather than overflow the stack (docs/44, epic-cc#662).
        device::Core::Pic18 if cli.outline => {
            let opts = outline::Options {
                stack_depth: device.stack_depth as usize,
                ir_depth: cg.max_depth,
                ..outline::Options::default()
            };
            let (asm, l) = outline::factor_with_locs(&asm, &locs, &opts);
            locs = l;
            asm
        }
        device::Core::Pic18 => asm,
        device::Core::PicBaseline => {
            isel_pic_baseline::verify_page_fit(&m, &asm, &addrs);
            // A const read CALLs its `__read_` entry, one stack level the
            // IR depth gate never sees. With a read present the deepest
            // frame plus `__start -> main` plus the reader must fit the
            // silicon stack, or the shift register drops the oldest
            // return address with no trap (D-5).
            if asm.contains("CALL __read_") && cg.max_depth + 1 > device.stack_depth as usize {
                diag::error(&format!(
                    "pic-baseline: const reads need a __read CALL level the {}-level stack cannot take at call depth {}",
                    device.stack_depth, cg.max_depth
                ));
            }
            asm
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
    // The fuse half comes from the compiled section on the `EPIC_CONFIG`
    // path, or from the recovered pragma pairs when clang dropped them.
    let fuse_spec = canonical_spec
        .as_deref()
        .or(pragma_spec.as_deref())
        .map(|s| driver::fosc::fuse_spec(s))
        .unwrap_or_default();
    let config_bytes: Option<Vec<u8>> = if canonical_spec.is_some() || pragma_spec.is_some() {
        let bytes = device::try_resolve_config(&device.config, &fuse_spec).unwrap_or_else(|e| {
            match &prescan_loc {
                Some((file, line, col)) => diag::error_at(file, *line, *col, &e),
                None => diag::error(&e),
            }
        });
        Some(bytes)
    } else {
        None
    };
    let program_words = asm::assemble_words(device, &asm);
    if let Some(lt_path) = &cli.line_table {
        std::fs::write(
            lt_path,
            driver::report::line_table_text(&device, &asm, &locs),
        )
        .expect("write line table");
    }
    if let Some(sc_path) = &cli.sidecar {
        let rows = driver::sidecar::line_rows(&asm, &locs);
        let dbg_vars = irparse::parse_debug_vars(&ll_text);
        std::fs::write(sc_path, driver::sidecar::encode(&rows, &layout, &dbg_vars))
            .expect("write sidecar");
    }
    let hex = driver::hex::emit(&device, &program_words, config_bytes.as_deref());
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
    if let Some(report_path) = &cli.report {
        let json = driver::report::report_json(
            &device,
            &layout,
            program_words.len(),
            config_bytes.as_deref(),
            fosc_hz,
        );
        std::fs::write(report_path, json).expect("write report");
    }
    std::fs::write(&cli.output, hex).expect("write hex");
}
