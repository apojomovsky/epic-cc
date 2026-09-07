//! SDCC parity differential harness (docs/35, P0).
//!
//! Compiles each corpus program with both epic-cc and SDCC, loads both
//! outputs into our simulator, seeds identical inputs, and compares the
//! program's named output globals (matched by name across the two compilers'
//! symbol/map output) plus cycle count. Records flash words + RAM bytes from
//! both. Never compares the whole RAM image: the two compilers allocate
//! globals and locals at different addresses with different overlay
//! strategies, so unrelated bytes would differ (mirrors the fuzz
//! differential, which compares a single named checksum global).
//!
//! SDCC is GPL and lives in the image as an external oracle only (ADR-006
//! boundary, docs/35 section 2). This crate never links or commits SDCC
//! code; it invokes `sdcc`/`gplink` as external processes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod corpus;

/// A corpus program: the C source plus the names of its volatile input and
/// output globals (matched by name across both compilers' maps).
#[derive(Debug, Clone)]
pub struct CorpusProgram {
    /// The C source text.
    pub source: String,
    /// Volatile input globals, seeded identically on both sides.
    pub inputs: Vec<Input>,
    /// Volatile output globals, compared by name after the run.
    pub outputs: Vec<String>,
}

/// One volatile input global: `volatile unsigned <width> <name>;`, seeded
/// with `value` on both sides of the differential.
#[derive(Debug, Clone)]
pub struct Input {
    pub name: String,
    pub width: u8, // 8, 16, or 32
    pub value: u32,
}

/// The measured result of one compiler on one program.
#[derive(Debug, Clone)]
pub struct CompilerResult {
    pub flash_words: usize,
    pub ram_bytes: usize,
    pub cycles: usize,
    /// Named output globals -> value, read from the sim after the run.
    pub outputs: HashMap<String, u32>,
}

/// The differential result for one program.
#[derive(Debug, Clone)]
pub struct DifferentialResult {
    pub program: String,
    pub epic: CompilerResult,
    pub sdcc: CompilerResult,
    /// True when every named output matches and both halted.
    pub pass: bool,
    /// Human-readable mismatch detail when `pass` is false.
    pub detail: String,
}

/// A per-run scratch directory in the OS temp dir (unique per process + call
/// so parallel runs never collide).
pub struct WorkDir {
    pub path: PathBuf,
}

impl WorkDir {
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "sdcc-parity-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&path).expect("create the parity work dir");
        WorkDir { path }
    }
}

/// Locate the epic-cc driver binary, mirroring the fuzz crate's pattern.
fn epic_cc_binary() -> PathBuf {
    if let Some(p) = std::env::var_os("PIC8_DRIVER") {
        return PathBuf::from(p);
    }
    if let Some(p) = option_env!("CARGO_BIN_EXE_epic-cc") {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().expect("current_exe");
    let mut dir = exe.clone();
    dir.pop();
    if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
        dir.pop();
    }
    dir.join("epic-cc")
}

/// The PIC clang binary (the pinned clang in the image).
fn pic_clang() -> String {
    std::env::var("PIC8_CLANG_UNWRAPPED").unwrap_or_else(|_| "/opt/clang/bin/clang".into())
}

/// The PIC clang resource dir.
fn pic_clang_resdir() -> String {
    std::env::var("PIC8_CLANG_RESOURCE_DIR").unwrap_or_else(|_| "/opt/clang/lib/clang/20".into())
}

/// Locate the SDCC binary (in the image at /usr/local/bin/sdcc, or
/// $PIC8_SDCC).
fn sdcc_binary() -> PathBuf {
    if let Some(p) = std::env::var_os("PIC8_SDCC") {
        return PathBuf::from(p);
    }
    PathBuf::from("/usr/local/bin/sdcc")
}

/// Locate gplink (ships with gputils, in the image).
fn gplink_binary() -> PathBuf {
    if let Some(p) = std::env::var_os("PIC8_GPLINK") {
        return PathBuf::from(p);
    }
    PathBuf::from("/usr/local/bin/gplink")
}

/// Compile a program with epic-cc, producing hex + map. Returns (hex_path,
/// map_path).
fn compile_epic(
    prog: &CorpusProgram,
    dir: &WorkDir,
    device: &device::Device,
) -> Result<(PathBuf, PathBuf, usize), String> {
    let c_path = dir.path.join("prog.c");
    std::fs::write(&c_path, &prog.source).map_err(|e| format!("write prog.c: {e}"))?;
    let hex_path = dir.path.join("epic.hex");
    let map_path = dir.path.join("epic.map");
    let driver = epic_cc_binary();
    let out = Command::new(&driver)
        .arg(&c_path)
        .arg("-o")
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .args(["--device", device.name])
        .env("PIC8_CLANG_UNWRAPPED", pic_clang())
        .env("PIC8_CLANG_RESOURCE_DIR", pic_clang_resdir())
        .output()
        .map_err(|e| format!("run epic-cc: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "epic-cc failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    // The driver prints the size report to stderr: `flash: N/8192 words`.
    // Parse the flash word count from it (the sim pads the hex to the full
    // device flash, so the hex length overcounts).
    let stderr = String::from_utf8_lossy(&out.stderr);
    let flash_words = stderr
        .lines()
        .find_map(|l| {
            let l = l.trim();
            l.strip_prefix("flash: ")
                .and_then(|r| r.split('/').next())
                .and_then(|n| n.trim().parse::<usize>().ok())
        })
        .ok_or_else(|| format!("no flash count in epic-cc stderr: {stderr}"))?;
    Ok((hex_path, map_path, flash_words))
}

/// Compile a program with SDCC, producing hex + map. SDCC's pic16 port
/// targets the PIC18 family; pic14 targets the classic 14-bit core. The
/// device flag differs per core. SDCC does the full link (producing the
/// hex); gplink is then re-run with `-m` on the same objects to produce the
/// symbol map (SDCC's own link does not emit a map).
fn compile_sdcc(
    prog: &CorpusProgram,
    dir: &WorkDir,
    device: &device::Device,
) -> Result<(PathBuf, PathBuf), String> {
    let c_path = dir.path.join("prog.c");
    std::fs::write(&c_path, &prog.source).map_err(|e| format!("write prog.c: {e}"))?;
    let sdcc = sdcc_binary();
    let gplink = gplink_binary();

    let (port, mcu) = match device.core {
        device::Core::Pic14 => ("pic14", device.name.to_lowercase()),
        device::Core::Pic18 => ("pic16", device.name.to_lowercase()),
        device::Core::Pic14e => return Err("pic14e has no SDCC parity target yet".into()),
    };
    // SDCC device names drop the leading `p` (p16f877a -> 16f877a).
    let sdcc_mcu = mcu.strip_prefix('p').unwrap_or(&mcu).to_string();

    // SDCC full link: compiles to .o, assembles, links to hex. The output
    // name is the source basename (prog.hex) in the work dir.
    let hex_path = dir.path.join("prog.hex");
    let out = Command::new(&sdcc)
        .arg(format!("-m{port}"))
        .arg(format!("-p{sdcc_mcu}"))
        .arg("--use-non-free")
        .arg(&c_path)
        .current_dir(&dir.path)
        .output()
        .map_err(|e| format!("run sdcc: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "sdcc failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    if !hex_path.exists() {
        return Err(format!(
            "sdcc produced no hex at {}: {}",
            hex_path.display(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    // Re-run gplink with -m on the same objects to get the symbol map. The
    // object is prog.o in the work dir; the libs are in the SDCC lib dirs.
    let obj = dir.path.join("prog.o");
    let map_path = dir.path.join("prog.map");
    let lib_dir = format!("/usr/local/share/sdcc/lib/{port}");
    let nonfree_dir = format!("/usr/local/share/sdcc/non-free/lib/{port}");
    let lib = format!("libsdcc.lib");
    // Device lib names differ per port: pic14 uses `pic16<device>.lib`
    // (e.g. pic16f877a.lib), pic16 uses `libdev<device>.lib`
    // (e.g. libdev18f4550.lib).
    let devlib = match port {
        // pic14 libs are `pic16<rest>.lib` where <rest> is the device name
        // minus the `p16` prefix (p16f877a -> pic16f877a.lib).
        "pic14" => format!("pic16{}.lib", device.name.trim_start_matches("p16")),
        "pic16" => format!("libdev{sdcc_mcu}.lib"),
        _ => return Err(format!("unknown SDCC port {port}")),
    };
    let out = Command::new(&gplink)
        .arg(format!("-I{lib_dir}"))
        .arg(format!("-I{nonfree_dir}"))
        .args(["-w", "-r", "-m", "-o"])
        .arg(&hex_path)
        .arg(&obj)
        .arg(&lib)
        .arg(&devlib)
        .current_dir(&dir.path)
        .output()
        .map_err(|e| format!("run gplink: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "gplink failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    if !map_path.exists() {
        return Err(format!(
            "gplink produced no map at {}: {}",
            map_path.display(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok((hex_path, map_path))
}

/// Parse an epic-cc map (`global <name> 0xNN`, `const <name>`,
/// `local <key> 0xNN`) into a name -> address map for globals.
fn parse_epic_map(map_text: &str) -> HashMap<String, u16> {
    let mut m = HashMap::new();
    for line in map_text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("global ") {
            let mut it = rest.split_whitespace();
            if let (Some(name), Some(addr)) = (it.next(), it.next()) {
                if let Ok(a) = u16::from_str_radix(addr.trim_start_matches("0x"), 16) {
                    m.insert(name.to_string(), a);
                }
            }
        }
    }
    m
}

/// Parse an SDCC/gplink map into a name -> address map. gplink's map has a
/// symbol table with lines like `name 0xADDR ...`; we match the symbol name
/// and its hex address. SDCC mangles C globals with a leading `_`, so we
/// strip it to match the source-level name.
fn parse_sdcc_map(map_text: &str) -> HashMap<String, u16> {
    let mut m = HashMap::new();
    for line in map_text.lines() {
        let line = line.trim();
        // gplink symbol lines: `<name> <addr> <size> <type> ...` where addr
        // is hex like 0x0A3F. Skip section headers and non-symbol lines.
        let mut it = line.split_whitespace();
        let name = it.next();
        let addr = it.next();
        if let (Some(name), Some(addr)) = (name, addr) {
            if let Some(hex) = addr.strip_prefix("0x") {
                if let Ok(a) = u16::from_str_radix(hex, 16) {
                    let clean = name.strip_prefix('_').unwrap_or(name);
                    m.insert(clean.to_string(), a);
                }
            }
        }
    }
    m
}

/// Parse the SDCC program size (in words) from the gplink map's Section Info
/// table. The `program`-located sections' highest (address + size) byte is
/// the program's code end; divide by 2 for the word count (both PIC14 and
/// PIC18 store one word per 2 hex bytes).
fn parse_sdcc_flash_words(map_text: &str) -> Option<usize> {
    let mut max_end: usize = 0;
    let mut in_sections = false;
    for line in map_text.lines() {
        let line = line.trim();
        if line.starts_with("Section Info") {
            in_sections = true;
            continue;
        }
        if in_sections {
            // Section rows: `<name> <type> <addr> <loc> <size>`. The
            // `program`-located rows are code; track the max end byte.
            let mut it = line.split_whitespace();
            let _name = it.next();
            let _type = it.next();
            let addr = it.next();
            let loc = it.next();
            let size = it.next();
            if let (Some(addr), Some(loc), Some(size)) = (addr, loc, size) {
                if loc == "program" {
                    if let (Ok(a), Ok(s)) = (
                        usize::from_str_radix(addr.trim_start_matches("0x"), 16),
                        usize::from_str_radix(size.trim_start_matches("0x"), 16),
                    ) {
                        max_end = max_end.max(a + s);
                    }
                }
            }
            // The symbol table follows the section table; stop there.
            if line.starts_with("gplink-") || line.starts_with("Map File") {
                break;
            }
        }
    }
    if max_end > 0 {
        Some(max_end / 2)
    } else {
        None
    }
}

/// Run one compiler's hex in our sim, seed inputs, read named outputs and
/// cycle count. `map` maps global name -> address for this compiler.
fn run_sim(
    hex_path: &Path,
    map: &HashMap<String, u16>,
    prog: &CorpusProgram,
    device: &device::Device,
) -> Result<CompilerResult, String> {
    let hex = std::fs::read_to_string(hex_path)
        .map_err(|e| format!("read {}: {e}", hex_path.display()))?;
    let max_steps = 5_000_000usize;

    let (cycles, ram) = match device.core {
        device::Core::Pic14 => {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
            for input in &prog.inputs {
                let addr = *map
                    .get(&input.name)
                    .ok_or_else(|| format!("no global '{}' in the map", input.name))?;
                seed_le(p.ram_mut(), addr, input.width, input.value);
            }
            // Run to a fixed step budget. SDCC's linked output does not
            // halt (its startup code loops after main returns), so we read
            // the named outputs after the budget rather than requiring
            // `halted()`. The outputs are stable once main has completed.
            let steps = p.run(max_steps);
            (steps, p.ram().to_vec())
        }
        device::Core::Pic18 => {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            for input in &prog.inputs {
                let addr = *map
                    .get(&input.name)
                    .ok_or_else(|| format!("no global '{}' in the map", input.name))?;
                seed_le(p.ram_mut(), addr, input.width, input.value);
            }
            let steps = p.run(max_steps);
            (steps, p.ram().to_vec())
        }
        device::Core::Pic14e => return Err("pic14e has no sim parity target yet".into()),
    };

    let mut outputs = HashMap::new();
    for name in &prog.outputs {
        let addr = *map
            .get(name)
            .ok_or_else(|| format!("no global '{name}' in the map"))?;
        // Outputs are read as the global's width; default to 1 byte unless
        // the input widths hint otherwise. The corpus declares output widths
        // via the input list convention: we read 1 byte for u8 outputs.
        outputs.insert(name.clone(), ram[addr as usize] as u32);
    }

    // Flash words: the program's assembled word count. For epic-cc, the
    // driver's stderr size report has it. For SDCC, the gplink map's
    // section info lists the program size; the hex is padded to the full
    // device flash, so the hex length overcounts.
    let flash_words = match device.core {
        device::Core::Pic14 => pic14_sim::parse_hex(&hex).len(),
        device::Core::Pic18 => pic14_sim::parse_hex_pic18(&hex).len(),
        device::Core::Pic14e => 0,
    };
    let ram_bytes = ram.len();

    Ok(CompilerResult {
        flash_words,
        ram_bytes,
        cycles,
        outputs,
    })
}

fn seed_le(ram: &mut [u8], addr: u16, width: u8, value: u32) {
    let bytes = match width {
        8 => 1,
        16 => 2,
        32 => 4,
        w => panic!("bad input width {w}"),
    };
    for i in 0..bytes {
        ram[addr as usize + i] = ((value >> (8 * i)) & 0xFF) as u8;
    }
}

/// Run the epic-cc side of the differential for one program (compile + sim),
/// returning the measured result. Exposed for testing the epic-cc path in
/// isolation when SDCC is not present.
pub fn run_epic(prog: &CorpusProgram, device: &device::Device) -> Result<CompilerResult, String> {
    let dir = WorkDir::new(
        &prog
            .outputs
            .first()
            .cloned()
            .unwrap_or_else(|| "prog".into()),
    );
    let (epic_hex, epic_map_path, flash_words) = compile_epic(prog, &dir, device)?;
    let epic_map_text =
        std::fs::read_to_string(&epic_map_path).map_err(|e| format!("read epic map: {e}"))?;
    let epic_map = parse_epic_map(&epic_map_text);
    let mut r = run_sim(&epic_hex, &epic_map, prog, device)?;
    r.flash_words = flash_words;
    Ok(r)
}

/// Run the full differential for one program on one device.
pub fn run_differential(
    prog: &CorpusProgram,
    device: &device::Device,
) -> Result<DifferentialResult, String> {
    let dir = WorkDir::new(
        &prog
            .outputs
            .first()
            .cloned()
            .unwrap_or_else(|| "prog".into()),
    );

    let (epic_hex, epic_map_path, epic_flash) = compile_epic(prog, &dir, device)?;
    let epic_map_text =
        std::fs::read_to_string(&epic_map_path).map_err(|e| format!("read epic map: {e}"))?;
    let epic_map = parse_epic_map(&epic_map_text);
    let mut epic = run_sim(&epic_hex, &epic_map, prog, device)?;
    epic.flash_words = epic_flash;

    let (sdcc_hex, sdcc_map_path) = compile_sdcc(prog, &dir, device)?;
    let sdcc_map_text =
        std::fs::read_to_string(&sdcc_map_path).map_err(|e| format!("read sdcc map: {e}"))?;
    let sdcc_map = parse_sdcc_map(&sdcc_map_text);
    let mut sdcc = run_sim(&sdcc_hex, &sdcc_map, prog, device)?;
    if let Some(w) = parse_sdcc_flash_words(&sdcc_map_text) {
        sdcc.flash_words = w;
    }

    // Compare named outputs.
    let mut pass = true;
    let mut detail = String::new();
    for name in &prog.outputs {
        let e = epic.outputs.get(name);
        let s = sdcc.outputs.get(name);
        match (e, s) {
            (Some(ev), Some(sv)) if ev == sv => {}
            (Some(ev), Some(sv)) => {
                pass = false;
                detail.push_str(&format!("output {name}: epic={ev:#x} sdcc={sv:#x}; "));
            }
            (None, _) => {
                pass = false;
                detail.push_str(&format!("output {name}: missing in epic; "));
            }
            (_, None) => {
                pass = false;
                detail.push_str(&format!("output {name}: missing in sdcc; "));
            }
        }
    }

    Ok(DifferentialResult {
        program: prog.outputs.first().cloned().unwrap_or_default(),
        epic,
        sdcc,
        pass,
        detail,
    })
}
