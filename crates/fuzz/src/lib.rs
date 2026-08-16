//! Seeded C generator + differential runner (PIC driver+sim vs host clang).
//!
//! Task 1 of milestone 14: the generator skeleton and the differential
//! harness. The generator emits a tiny, deterministic C program in the
//! milestone's "discipline" (unsigned-only arithmetic in genuinely
//! explicit-width types — `u8`/`u16`/`u32` from `TYPEDEF_PROLOGUE`, never
//! bare `unsigned long`, which is 64-bit on LP64 hosts — guarded shifts, a
//! volatile `u8` checksum); the differential runner compiles it twice —
//! through the PIC8 driver into `pic14-sim`, and through host clang into a
//! native binary — seeds the volatile inputs identically on both sides, and
//! compares the resulting checksums.
//!
//! The harness contracts (see docs/27-phase6-random-testing-plan.md):
//! - `generate(seed)` is deterministic (seeded RNG, no entropy);
//! - the C discipline keeps host and PIC semantics identical, so a checksum
//!   mismatch (or a non-halting sim, or a compiler panic) is a real bug;
//! - the PIC side mirrors `crates/driver/tests/long_e2e.rs`: the volatile
//!   globals' addresses come from the same alloc layout the driver used, the
//!   driver binary (a workspace member) produces the hex, `pic14-sim` runs
//!   it, and the machine must halt;
//! - the host side compiles `prog.c` (+ a generated `host_main.c` that seeds
//!   the inputs by name) with the nix shell's `clang` (the pinned clang
//!   WITHOUT `-target`; the unwrapped `$PIC8_CLANG_UNWRAPPED` cannot find the
//!   host's stdio.h) and reads the printed checksum.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// The volatile checksum global's name (fixed by the C discipline).
pub const CHECKSUM_NAME: &str = "checksum";

/// The explicit-width typedef prologue emitted at the top of every generated
/// program.
///
/// WHY (the milestone's "Important" fix): Task 1 documented `unsigned long`
/// as "32-bit on both msp430 and the host" — that equivalence is FALSE. On
/// LP64 hosts `unsigned long` is 64-bit, so a u32 computation whose result
/// exceeds 2^32 (e.g. `x * x` for x = 0xFFFFFFFF, or a 64-bit quotient)
/// diverges: msp430 wraps at 2^32, the host does not. `stdint.h` was the
/// first choice (`uint8_t`/`uint16_t`/`uint32_t` are exactly this), but it
/// does NOT resolve under the driver's fixed flags — `-nostdinc` drops the
/// builtin resource-dir include path (verified empirically; adding an
/// explicit `-isystem` fixes it, but the driver's flags are not ours to
/// change). So the robust option is self-contained typedefs guarded on the
/// target macro clang defines for the msp430 triple:
///
/// - u8  = unsigned char  (8 bits on both targets)
/// - u16 = unsigned short (16 bits on both targets)
/// - u32 = msp430: `unsigned long` (msp430 int is 16-bit, so its 32-bit
///        type is long) / host: `unsigned int` (32-bit on the pinned
///        x86-64-linux host) — genuinely 32-bit on BOTH sides.
///
/// With these, u8/u16/u32 arithmetic wraps identically on both sides and
/// the differential is meaningful for values beyond 2^16 (pinned by
/// `u32_arithmetic_wraps_identically_on_both_sides` in tests/differential.rs
/// and by `unsigned_long_u32_arithmetic_mismatches`, which shows the old
/// discipline failing).
pub const TYPEDEF_PROLOGUE: &str = "\
#ifdef __MSP430__\n\
typedef unsigned char u8;\n\
typedef unsigned short u16;\n\
typedef unsigned long u32;\n\
#else\n\
typedef unsigned char u8;\n\
typedef unsigned short u16;\n\
typedef unsigned int u32;\n\
#endif\n";

/// The volatile input globals' name prefix (`in0`, `in1`, …).
const INPUT_PREFIX: &str = "in";

/// Sim step budget per differential run (the long e2e uses the same).
const MAX_SIM_STEPS: usize = 5_000_000;

// ---------------------------------------------------------------------------
// Seeded RNG
// ---------------------------------------------------------------------------

/// SplitMix64 — a small, self-contained, deterministic 64-bit PRNG.
///
/// Chosen over a bare LCG at the same zero-dependency cost: SplitMix64 is a
/// few lines, keeps only a 64-bit word of state, and — unlike an LCG, whose
/// low bits cycle visibly — mixes every output bit, so adjacent seeds produce
/// meaningfully different programs while the output stays perfectly
/// reproducible (the corpus contract; no entropy is ever consulted).
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` for small `n` (modulo bias is irrelevant here).
    fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % u64::from(n)) as u32
    }
}

// ---------------------------------------------------------------------------
// Program model
// ---------------------------------------------------------------------------

/// One volatile input global: `volatile unsigned <width> <name>;`, seeded
/// with `value` (the low `width` bits) on both sides of the differential.
#[derive(Debug, Clone)]
pub struct Input {
    pub name: String,
    pub value: u32,
    pub width: u8, // 8 | 16 | 32
}

/// A generated program: the C source plus the metadata the differential
/// harness needs to seed and observe it.
#[derive(Debug, Clone)]
pub struct Program {
    pub c_source: String,
    pub inputs: Vec<Input>,
    pub checksum_name: String,
}

// ---------------------------------------------------------------------------
// Generator (Task 1: a few scalar expression shapes)
// ---------------------------------------------------------------------------

/// Generate a deterministic program from `seed`.
///
/// Task-1 surface: 2–3 `volatile u8` inputs and one scalar expression over
/// them, computed in `u32` space (genuinely 32-bit on both msp430 and the
/// host — see `TYPEDEF_PROLOGUE`; never `unsigned long`, which is 64-bit on
/// LP64 hosts) and folded into the `u8` checksum with an explicit narrowing
/// cast. The expression shape and its constants come from the seeded RNG, so
/// output varies across seeds yet reproduces exactly.
pub fn generate(seed: u64) -> Program {
    let mut rng = SplitMix64::new(seed);
    let n = 2 + rng.below(2) as usize; // 2..=3 inputs
    let mut inputs = Vec::with_capacity(n);
    let mut decls = String::new();
    for i in 0..n {
        let name = format!("{INPUT_PREFIX}{i}");
        decls.push_str(&format!("volatile u8 {name};\n"));
        inputs.push(Input {
            name,
            value: rng.next_u64() as u32,
            width: 8,
        });
    }
    let a = &inputs[0].name;
    let b = &inputs[1].name;
    let shape = rng.below(4);
    let k1 = [2u32, 3, 5, 7][rng.below(4) as usize];
    let k2 = rng.below(16) as u32;
    let s = rng.below(8) as u32; // < 8: the shift stays inside the u8 value
    let expr = match shape {
        // All four keep every intermediate in u32 (32-bit on both targets),
        // so the wrap and the final (u8) truncation are defined on both.
        0 => format!("((u32){a} * {k1}u + {k2}u)"),
        1 => format!("(((u32){a} << {s}u) ^ (u32){b})"),
        2 => format!("((u32){a} + (u32){b} * {k1}u)"),
        _ => format!("((((u32){a} * {k1}u) + (u32){b}) >> {s}u)"),
    };
    let c_source = format!(
        "{TYPEDEF_PROLOGUE}{decls}volatile u8 {checksum};\n\
         void main(void) {{\n  {checksum} = (u8)({expr});\n}}\n",
        checksum = CHECKSUM_NAME
    );
    Program {
        c_source,
        inputs,
        checksum_name: CHECKSUM_NAME.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Differential runner
// ---------------------------------------------------------------------------

/// Run the program on both sides and return the agreed checksum, or a
/// description of the failure: a compile/driver error (including a compiler
/// panic, which surfaces as a failed process or a caught pipeline panic), a
/// non-halting sim run, or a host/PIC checksum mismatch.
pub fn run_differential(program: &Program) -> Result<u32, String> {
    let dir = WorkDir::new();
    let c_path = dir.path.join("prog.c");
    std::fs::write(&c_path, &program.c_source)
        .map_err(|e| format!("write prog.c: {e}"))?;

    let pic = run_pic(program, &c_path, &dir)?;
    let host = run_host(program, &c_path, &dir)?;

    if pic == host {
        Ok(pic)
    } else {
        Err(format!("mismatch: pic checksum {pic}, host checksum {host}"))
    }
}

/// PIC side: alloc layout (in-process, mirroring the driver's e2e) for the
/// input/checksum addresses, the driver binary for the hex, `pic14-sim`
/// seeded at those addresses, run, checksum read, `halted()` required.
fn run_pic(program: &Program, c_path: &Path, dir: &WorkDir) -> Result<u32, String> {
    let layout = pic_layout(c_path)?;
    let checksum_addr = *layout
        .globals
        .get(&program.checksum_name)
        .ok_or_else(|| format!("no global '{}' in the alloc map", program.checksum_name))?;

    let hex_path = dir.path.join("prog.hex");
    run_driver(c_path, &hex_path)?;

    let hex =
        std::fs::read_to_string(&hex_path).map_err(|e| format!("read {}: {e}", hex_path.display()))?;
    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
    for input in &program.inputs {
        let addr = *layout
            .globals
            .get(&input.name)
            .ok_or_else(|| format!("no global '{}' in the alloc map", input.name))?;
        seed_le(&mut p, addr, input.width, input.value);
    }
    p.run(MAX_SIM_STEPS);
    if !p.halted() {
        return Err(format!("simulator did not halt within {MAX_SIM_STEPS} steps"));
    }
    Ok(read_le(p.ram(), checksum_addr, 1) as u32)
}

/// Host side: compile `prog.c` + a generated `host_main.c` with host clang
/// (no `-target`), run the native binary, parse the printed checksum.
fn run_host(program: &Program, c_path: &Path, dir: &WorkDir) -> Result<u32, String> {
    let hm_path = dir.path.join("host_main.c");
    std::fs::write(&hm_path, host_main_source(program)?)
        .map_err(|e| format!("write host_main.c: {e}"))?;

    let clang = host_clang();
    let obj_prog = dir.path.join("prog_pic.o");
    let obj_host = dir.path.join("host_main.o");
    let exe = dir.path.join("prog");

    // `-Dmain=pic_main` renames the generated `main`, so it must apply to
    // prog.c only — host_main.c provides the real `main` and is compiled
    // without the rename, then the two objects are linked.
    run_ok(
        Command::new(&clang)
            .args(["-O1", "-Dmain=pic_main", "-c"])
            .arg(c_path)
            .arg("-o")
            .arg(&obj_prog),
        "host clang (prog.c)",
    )?;
    run_ok(
        Command::new(&clang)
            .args(["-O1", "-c"])
            .arg(&hm_path)
            .arg("-o")
            .arg(&obj_host),
        "host clang (host_main.c)",
    )?;
    run_ok(
        Command::new(&clang)
            .args(["-O1"])
            .arg(&obj_prog)
            .arg(&obj_host)
            .arg("-o")
            .arg(&exe),
        "host clang (link)",
    )?;

    let out = Command::new(&exe)
        .output()
        .map_err(|e| format!("run the host binary: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "host binary failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().next().ok_or("host binary printed nothing")?;
    line.trim()
        .parse::<u32>()
        .map_err(|_| format!("host binary printed a non-checksum line: {stdout:?}"))
}

/// The generated `host_main.c`: seeds each input global by name (matching the
/// sim-side RAM seeding byte-for-byte), calls the renamed `pic_main`, and
/// prints the checksum as a bare unsigned decimal.
fn host_main_source(program: &Program) -> Result<String, String> {
    // host_main.c is a separate translation unit: it needs the typedef
    // prologue itself so `extern volatile u32 in0;` matches the generated
    // globals' typedef'd types.
    let mut s = String::from("#include <stdio.h>\n");
    s.push_str(TYPEDEF_PROLOGUE);
    for input in &program.inputs {
        s.push_str(&format!(
            "extern volatile {} {};\n",
            width_type(input.width)?,
            input.name
        ));
    }
    s.push_str(&format!(
        "extern volatile unsigned char {};\n",
        program.checksum_name
    ));
    s.push_str("void pic_main(void);\nint main(void) {\n");
    for input in &program.inputs {
        s.push_str(&format!(
            "  {} = 0x{:X}u;\n",
            input.name,
            input.value & width_mask(input.width)
        ));
    }
    s.push_str(&format!(
        "  pic_main();\n  printf(\"%u\\n\", (unsigned){});\n  return 0;\n}}\n",
        program.checksum_name
    ));
    Ok(s)
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

/// The PIC clang pair (`$PIC8_CLANG_UNWRAPPED` + `$PIC8_CLANG_RESOURCE_DIR`),
/// which the driver and the in-process layout pipeline both require.
fn pic_clang() -> Result<(String, String), String> {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED")
        .map_err(|_| "PIC8_CLANG_UNWRAPPED is not set (run inside `nix develop`)".to_string())?;
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").map_err(|_| {
        "PIC8_CLANG_RESOURCE_DIR is not set (run inside `nix develop`)".to_string()
    })?;
    Ok((clang, resdir))
}

/// The host clang: the nix shell's plain `clang` (the pinned clang WITHOUT
/// `-target`, whose wrapper knows the host toolchain — the unwrapped
/// `$PIC8_CLANG_UNWRAPPED` cannot find the host's stdio.h, verified during
/// development). `PIC8_HOST_CLANG` overrides it.
fn host_clang() -> String {
    std::env::var("PIC8_HOST_CLANG").unwrap_or_else(|_| "clang".to_string())
}

/// The volatile globals' addresses: run the same pipeline the driver runs
/// (mirroring `crates/driver/tests/long_e2e.rs`). Panics in the pipeline
/// (a compiler bug) are caught and reported as a failure, so the fuzz loop
/// survives them.
fn pic_layout(c_path: &Path) -> Result<alloc::AllocLayout, String> {
    let (clang, resdir) = pic_clang()?;
    let ll = Command::new(&clang)
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
        ])
        .arg(c_path)
        .output()
        .map_err(|e| format!("run clang for the layout: {e}"))?;
    if !ll.status.success() {
        return Err(format!(
            "clang (layout) failed: {}",
            String::from_utf8_lossy(&ll.stderr)
        ));
    }
    let ll_text = String::from_utf8(ll.stdout).map_err(|e| format!("clang stdout: {e}"))?;
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut m = irparse::parse_ll(&ll_text);
        m = wholeprog::merge(m);
        m = legalize::legalize(m);
        let cg = callgraph::build(&m);
        alloc::allocate(&m, &callgraph::edges_text(&cg))
    }))
    .map_err(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| p.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("unknown panic");
        format!("compiler pipeline panic: {msg}")
    })
}

/// Run the driver binary (a workspace member) over the C file to produce the
/// hex, passing the PIC clang env vars it expects.
fn run_driver(c_path: &Path, hex_path: &Path) -> Result<(), String> {
    let (clang, resdir) = pic_clang()?;
    let driver = driver_binary()?;
    let out = Command::new(&driver)
        .arg(c_path)
        .arg(hex_path)
        .env("PIC8_CLANG_UNWRAPPED", &clang)
        .env("PIC8_CLANG_RESOURCE_DIR", &resdir)
        .output()
        .map_err(|e| format!("run the driver: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "driver failed (a compiler panic or an unsupported construct): {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Locate the driver binary, mirroring the driver crate's e2e pattern.
///
/// The e2e tests (inside `crates/driver`) use `env!("CARGO_BIN_EXE_driver")`,
/// which Cargo sets only for the package that owns the binary; this crate
/// instead finds the driver next to the running test executable in
/// `target/<profile>/` (the driver is a workspace member), honoring a
/// `PIC8_DRIVER` env override first. A bare `cargo test -p fuzz` that skipped
/// building other members builds the driver on demand — the nested `cargo`
/// cannot deadlock on the build lock because tests run only after the outer
/// build has finished (verified empirically).
fn driver_binary() -> Result<PathBuf, String> {
    static CACHE: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            if let Some(p) = std::env::var_os("PIC8_DRIVER") {
                return Ok(PathBuf::from(p));
            }
            if let Some(p) = option_env!("CARGO_BIN_EXE_driver") {
                return Ok(PathBuf::from(p));
            }
            let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
            let mut dir = exe.clone();
            dir.pop(); // the exe name
            if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
                dir.pop(); // integration-test binaries live in target/<profile>/deps
            }
            let candidate = dir.join("driver");
            if candidate.exists() {
                return Ok(candidate);
            }
            let mut cmd = Command::new("cargo");
            cmd.args(["build", "-p", "driver", "--quiet"]);
            if let Ok(profile) = std::env::var("PROFILE") {
                if profile != "debug" {
                    cmd.args(["--profile", &profile]);
                }
            }
            let status = cmd
                .status()
                .map_err(|e| format!("cargo build -p driver: {e}"))?;
            if !status.success() {
                return Err("cargo build -p driver failed".into());
            }
            if candidate.exists() {
                Ok(candidate)
            } else {
                Err(format!("driver binary not found at {}", candidate.display()))
            }
        })
        .clone()
}

fn run_ok(cmd: &mut Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed: {}", String::from_utf8_lossy(&out.stderr)))
    }
}

/// The C type name for a width under the typedef discipline (the host_main.c
/// externs must match the generated globals' typedef'd types exactly).
fn width_type(width: u8) -> Result<&'static str, String> {
    match width {
        8 => Ok("u8"),
        16 => Ok("u16"),
        32 => Ok("u32"),
        w => Err(format!("bad input width {w}")),
    }
}

fn width_mask(width: u8) -> u32 {
    match width {
        8 => 0xFF,
        16 => 0xFFFF,
        32 => 0xFFFF_FFFF,
        w => panic!("bad input width {w}"),
    }
}

/// Seed `width` little-endian bytes of `value` at `addr` (the sim side of
/// the harness's identical seeding; the host side uses `host_main_source`).
fn seed_le(p: &mut pic14_sim::Pic14, addr: u16, width: u8, value: u32) {
    let bytes = match width {
        8 => 1,
        16 => 2,
        32 => 4,
        w => panic!("bad input width {w}"),
    };
    for i in 0..bytes {
        p.ram_mut()[addr as usize + i] = ((value >> (8 * i)) & 0xFF) as u8;
    }
}

fn read_le(ram: &[u8; 512], addr: u16, bytes: u8) -> u32 {
    let mut v = 0u32;
    for i in 0..bytes as usize {
        v |= (ram[addr as usize + i] as u32) << (8 * i);
    }
    v
}

/// A per-run scratch directory in the OS temp dir (unique per process + call
/// so parallel tests never collide).
struct WorkDir {
    path: PathBuf,
}

static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);

impl WorkDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pic8-fuzz-{}-{}",
            std::process::id(),
            WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create the fuzz work dir");
        WorkDir { path }
    }
}
