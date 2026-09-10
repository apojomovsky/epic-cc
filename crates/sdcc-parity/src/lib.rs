//! SDCC parity differential harness (docs/35, P0).
//!
//! Compiles each corpus program with both epic-cc and SDCC, runs both
//! images in our simulator to the program's explicit `sleep` halt, and
//! compares the program's named output globals (matched by name across
//! the two compilers' symbol/map output) plus cycle count. Flash words
//! come from each side's final Intel HEX (whole image, identical
//! definition); RAM bytes are the live (non-zero) RAM bytes at the halt,
//! the only definition computable identically for two allocators that
//! place everything differently. Never compares the whole RAM image.
//!
//! SDCC is GPL and lives in the image as an external oracle only (ADR-006
//! boundary, docs/35 section 2). This crate never links or commits SDCC
//! code; it invokes `sdcc`/`gplink` as external processes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod corpus;

/// A corpus program: the C source plus the names of its volatile output
/// globals (matched by name across both compilers' maps). Inputs are part
/// of the source: SDCC's PIC18 crt0 clears all of BSS before `main`, so a
/// value written into RAM ahead of the run never survives to the program.
#[derive(Debug, Clone)]
pub struct CorpusProgram {
    /// Stable program name, keying the sdcc-known-bugs table.
    pub name: String,
    /// The C source text. `main` must end in `__asm__("sleep")`, the
    /// simulator's halt condition on every core.
    pub source: String,
    /// Volatile output globals, compared by name after the run.
    pub outputs: Vec<String>,
    /// The hand-computed value of the first output (docs/35 section 5
    /// item 2): the arbiter on a differential mismatch and the
    /// conformance target epic-cc must hit even where SDCC cannot run.
    /// Every corpus output is one byte by construction.
    pub expected: u8,
    /// Devices the program runs on, by device name (`p16f877a`). Empty
    /// means every device: most corpus programs are plain C that both
    /// compilers accept anywhere. A probe pinned to one family's SFR
    /// addresses (the data-EEPROM register file differs per family)
    /// lists exactly those devices instead of `#if`-ing on a compiler
    /// macro, which the corpus contract bans.
    pub devices: Vec<String>,
}

impl CorpusProgram {
    /// Whether this program runs on the named device.
    pub fn runs_on(&self, device_name: &str) -> bool {
        self.devices.is_empty() || self.devices.iter().any(|d| d == device_name)
    }
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
) -> Result<(PathBuf, PathBuf), String> {
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
    Ok((hex_path, map_path))
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
        // SDCC's pic14 port covers the Enhanced core (16F193x) via the
        // `libsdcce` library (docs/35 section 3 PIC14E table). The device
        // libs live in the non-free dir as `pic16<rest>.lib`, same naming
        // as classic pic14.
        device::Core::Pic14e => ("pic14", device.name.to_lowercase()),
        // SDCC has no baseline PIC support at all (docs/35 / docs/37), so a
        // baseline device never reaches the SDCC runner.
        device::Core::PicBaseline => panic!(
            "sdcc-parity: {} is pic-baseline; SDCC cannot target this core",
            device.name
        ),
    };
    // SDCC device names drop the leading `p` (p16f877a -> 16f877a).
    let sdcc_mcu = mcu.strip_prefix('p').unwrap_or(&mcu).to_string();

    // SDCC full link: compiles to .o, assembles, links to hex. The output
    // name is the source basename (prog.hex) in the work dir.
    let hex_path = dir.path.join("prog.hex");
    // SDCC's own link pulls the core and C libraries automatically but
    // not libm: a program using math.h needs an explicit `-l` with the
    // full archive name (pic16 `libm18f.lib`, pic14 `libm.lib`, Enhanced
    // `libme.lib`). Unused members are never pulled, so naming it for
    // every program is harmless.
    let mathlib = match device.core {
        device::Core::Pic18 => "libm18f.lib",
        device::Core::Pic14 => "libm.lib",
        device::Core::Pic14e => "libme.lib",
        device::Core::PicBaseline => panic!(
            "sdcc-parity: {} is pic-baseline; SDCC cannot target this core",
            device.name
        ),
    };
    let out = Command::new(&sdcc)
        .arg(format!("-m{port}"))
        .arg(format!("-p{sdcc_mcu}"))
        .arg("--use-non-free")
        .arg(format!("-l{mathlib}"))
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
    // The `-o` output goes to a THROWAWAY hex (maponly.hex), NOT prog.hex:
    // SDCC's own link already produced the correct prog.hex (with the crt0
    // startup that sets up the stack), and gplink's re-link here omits the
    // crt0 object, so writing it over prog.hex would clobber the good hex
    // with a broken one (no reset-vector startup, uninitialized FSRs).
    let obj = dir.path.join("prog.o");
    let map_path = dir.path.join("maponly.map");
    let map_hex = dir.path.join("maponly.hex");
    let lib_dir = format!("/usr/local/share/sdcc/lib/{port}");
    let nonfree_dir = format!("/usr/local/share/sdcc/non-free/lib/{port}");
    // SDCC's pic14 port links `libsdcc.lib` for classic parts and
    // `libsdcce.lib` for the Enhanced core (16F193x, docs/35 section 3
    // PIC14E table); pic16 always uses `libsdcc.lib`.
    let lib = match device.core {
        device::Core::Pic14e => "libsdcce.lib".to_string(),
        _ => "libsdcc.lib".to_string(),
    };
    // The pic16 C library (`libc18f.lib`: printf and friends) and the
    // math library (`libm18f.lib`) are separate archives SDCC's own link
    // pulls in automatically. The map-only re-link below must name them
    // explicitly, or any program using libc (the `%f` probe) or libm (the
    // `math` probe) fails the re-link with an unresolved symbol even
    // though SDCC's own link succeeded. pic14 links `libm.lib` the same
    // way; the Enhanced core's math lives in `libme.lib`.
    let clibs: Vec<&str> = match device.core {
        device::Core::Pic18 => vec!["libc18f.lib", "libm18f.lib"],
        device::Core::Pic14 => vec!["libm.lib"],
        device::Core::Pic14e => vec!["libme.lib"],
        device::Core::PicBaseline => panic!(
            "sdcc-parity: {} is pic-baseline; SDCC cannot target this core",
            device.name
        ),
    };
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
    let mut cmd = Command::new(&gplink);
    cmd.arg(format!("-I{lib_dir}"))
        .arg(format!("-I{nonfree_dir}"))
        .args(["-w", "-r", "-m", "-o"])
        .arg(&map_hex)
        .arg(&obj)
        .arg(&lib);
    for clib in &clibs {
        cmd.arg(clib);
    }
    let out = cmd
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

/// Program-memory words covered by the image's HEX data records,
/// identical for both compilers: the extent of data records inside the
/// program region (bytes below device.flash_words * 2), in words. Config
/// words, ID locations and EEPROM data sit above that region on every
/// supported device, so extended-address records keep them out without
/// special cases. Replaces two asymmetric counts (epic's assembled-code
/// report, SDCC's own-code-only map sections) that the first baseline
/// flagged as apples-to-oranges.
fn image_program_words(hex_text: &str, device: &device::Device) -> Result<usize, String> {
    let flash_bytes = device.flash_words as usize * 2;
    let mut upper = 0usize;
    let mut max_word = 0usize;
    let mut any = false;
    for line in hex_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let b = ihex_record(line)?;
        let len = b[0] as usize;
        let addr = ((b[1] as usize) << 8) | b[2] as usize;
        match b[3] {
            0x00 => {
                for i in 0..len {
                    let byte_addr = upper + addr + i;
                    if byte_addr < flash_bytes {
                        max_word = max_word.max(byte_addr / 2 + 1);
                        any = true;
                    }
                }
            }
            0x01 => break,
            0x04 => upper = ((b[4] as usize) << 8 | b[5] as usize) << 16,
            other => return Err(format!("unsupported HEX record type {other:#x}")),
        }
    }
    if any {
        Ok(max_word)
    } else {
        Err("no program data records in hex".to_string())
    }
}

/// Decode one Intel HEX line (`:LLAAAATT...`) into its info+data bytes.
fn ihex_record(line: &str) -> Result<Vec<u8>, String> {
    let hex = line
        .strip_prefix(':')
        .ok_or_else(|| format!("not Intel HEX: {line}"))?;
    let b = hex.as_bytes();
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    while i + 1 < b.len() {
        let hi = (b[i] as char)
            .to_digit(16)
            .ok_or_else(|| format!("bad hex digit in {line}"))?;
        let lo = (b[i + 1] as char)
            .to_digit(16)
            .ok_or_else(|| format!("bad hex digit in {line}"))?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Ok(out)
}

/// Tool versions stamped into every output so a baseline row is
/// reproducible (docs/35 section 7, pinning).
pub fn tool_versions() -> String {
    let epic = Command::new(epic_cc_binary())
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "epic-cc <unknown>".to_string());
    let sdcc = first_line(Command::new(sdcc_binary()).arg("-V").output().ok());
    let gplink = first_line(Command::new(gplink_binary()).arg("--version").output().ok());
    format!("{epic}; {sdcc}; {gplink}")
}

fn first_line(out: Option<std::process::Output>) -> String {
    let Some(o) = out else {
        return "<unavailable>".to_string();
    };
    let mut text = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if text.is_empty() {
        text = String::from_utf8_lossy(&o.stderr).trim().to_string();
    }
    text.lines().next().unwrap_or("<unavailable>").to_string()
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

/// Run one compiler's hex in our sim to the program's `sleep` halt, then
/// read the named outputs, the cycle count, and the live RAM bytes.
/// `map` maps global name -> address for this compiler.
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
            let steps = p.run(max_steps);
            require_halt(p.halted(), device, max_steps)?;
            (steps, p.ram().to_vec())
        }
        device::Core::Pic18 => {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            let steps = p.run(max_steps);
            require_halt(p.halted(), device, max_steps)?;
            (steps, p.ram().to_vec())
        }
        device::Core::Pic14e => {
            let mut p = pic14_sim::Pic14e::with_device(device, pic14_sim::parse_hex(&hex));
            let steps = p.run(max_steps);
            require_halt(p.halted(), device, max_steps)?;
            (steps, p.ram().to_vec())
        }
        device::Core::PicBaseline => panic!(
            "sdcc-parity: {} is pic-baseline; SDCC cannot target this core",
            device.name
        ),
    };

    // Live RAM bytes at the halt: the only RAM measure computable
    // identically for two allocators that place globals and locals at
    // different addresses (docs/35 section 4 comparison protocol).
    let ram_bytes = ram.iter().filter(|b| **b != 0).count();

    let mut outputs = HashMap::new();
    for name in &prog.outputs {
        let addr = *map
            .get(name)
            .ok_or_else(|| format!("no global '{name}' in the map"))?;
        // Every corpus output is one byte wide by construction.
        outputs.insert(name.clone(), ram[addr as usize] as u32);
    }

    Ok(CompilerResult {
        flash_words: image_program_words(&hex, device)?,
        ram_bytes,
        cycles,
        outputs,
    })
}

/// The step budget is a guard, not the norm: a corpus program halts in
/// `sleep`. A budget exhaustion is its own failure class (a runaway or an
/// unimplemented opcode), never a measurement.
fn require_halt(halted: bool, device: &device::Device, max_steps: usize) -> Result<(), String> {
    if halted {
        Ok(())
    } else {
        Err(format!("{}: no halt within {max_steps} steps", device.name))
    }
}

/// Run the epic-cc side of the differential for one program (compile + sim),
/// returning the measured result. Exposed for testing the epic-cc path in
/// isolation when SDCC is not present.
pub fn run_epic(prog: &CorpusProgram, device: &device::Device) -> Result<CompilerResult, String> {
    let dir = WorkDir::new(&prog.name);
    let (epic_hex, epic_map_path) = compile_epic(prog, &dir, device)?;
    let epic_map_text =
        std::fs::read_to_string(&epic_map_path).map_err(|e| format!("read epic map: {e}"))?;
    let epic_map = parse_epic_map(&epic_map_text);
    run_sim(&epic_hex, &epic_map, prog, device)
}

/// Run the full differential for one program on one device.
pub fn run_differential(
    prog: &CorpusProgram,
    device: &device::Device,
) -> Result<DifferentialResult, String> {
    let dir = WorkDir::new(&prog.name);

    let (epic_hex, epic_map_path) = compile_epic(prog, &dir, device)?;
    let epic_map_text =
        std::fs::read_to_string(&epic_map_path).map_err(|e| format!("read epic map: {e}"))?;
    let epic_map = parse_epic_map(&epic_map_text);
    let epic = run_sim(&epic_hex, &epic_map, prog, device)?;

    let (sdcc_hex, sdcc_map_path) = compile_sdcc(prog, &dir, device)?;
    let sdcc_map_text =
        std::fs::read_to_string(&sdcc_map_path).map_err(|e| format!("read sdcc map: {e}"))?;
    let sdcc_map = parse_sdcc_map(&sdcc_map_text);
    let sdcc = run_sim(&sdcc_hex, &sdcc_map, prog, device)?;

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
        program: prog.name.clone(),
        epic,
        sdcc,
        pass,
        detail,
    })
}
