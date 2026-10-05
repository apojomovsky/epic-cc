//! Hand-rolled argument parsing. The workspace has no external crates and
//! keeps it that way, so there is no `clap` here.

/// Which stage's text artifact to write instead of HEX. The pipeline's stage
/// boundaries are diffable text by design; this exposes them to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emit {
    Ll,
    Ir,
    Asm,
    Hex,
}

/// Optimization profile. The spellings match what PIC users already type;
/// `-Os` is the default and keeps today's size-first pipeline byte for
/// byte. Profiles select our own passes only: clang stays pinned at `-O1`
/// (the input-format contract), whatever the profile says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptLevel {
    /// Whole-program constants folded, control flow untouched: no loop
    /// restructure, no cross-function folding, no code factoring. The IR
    /// stays closest to clang's output, for debugger stepping and
    /// miscompile bisection, while real programs still fit in flash.
    O0,
    /// The curated pass list without any cross-function folding, code
    /// factoring still on: bisects the always-inline step of `-Os`.
    O1,
    /// Speed: no code factoring, and single-call-site callees fold even
    /// into `main`/ISR roots within a frame budget. Costs flash and
    /// possibly RAM; later speed levers land here.
    O2,
    /// Today's pipeline exactly: curated passes, single-call-site folds
    /// into ordinary callers, code factoring on.
    #[default]
    Os,
}

impl OptLevel {
    /// The report/JSON spelling (`-Os` renders `"Os"`).
    pub fn as_str(self) -> &'static str {
        match self {
            OptLevel::O0 => "O0",
            OptLevel::O1 => "O1",
            OptLevel::O2 => "O2",
            OptLevel::Os => "Os",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Cli {
    pub inputs: Vec<String>,
    pub output: String,
    pub includes: Vec<String>,
    pub defines: Vec<String>,
    pub device: String,
    pub emit: Emit,
    pub save_temps: Option<String>,
    pub verbose: bool,
    pub map: Option<String>,
    pub line_table: Option<String>,
    pub var_table: Option<String>,
    pub sidecar: Option<String>,
    pub report: Option<String>,
    /// Board clock in Hz (`board_build.f_cpu`); the last-resort D-4 source.
    pub f_cpu: Option<u64>,
    /// Optimization profile (`-O0`/`-O1`/`-O2`/`-Os`); `-Os` default.
    pub opt_level: OptLevel,
    /// PIC18 code factoring (docs/44); `--no-outline` turns it off.
    pub outline: bool,
    /// PIC14 pooled flash string table (epic-cc#815, addresses and
    /// gating per epic-cc#816); `--const-pool` turns it on. Off by
    /// default: the pool is additive, so unflagged output stays
    /// bit-identical.
    pub const_pool: bool,
}

pub const USAGE: &str = "\
usage: epic-cc [options] <input.c|input.cpp>...

  -o <file>            output file (default: a.hex)
  -I <dir>             include path, repeatable, forwarded to clang
  -D <name[=value]>    define, repeatable, forwarded to clang
  --target <name>      device name (p16f877a, 16F877A and PIC16F877A all resolve);
                       aliases: --device, --mcu, -mcu
  --emit <stage>       ll | ir | asm | hex (default: hex)
  --save-temps <dir>   write every stage artifact into <dir>
  --map <file>         write the symbol-to-address map (globals and
                       {func}::{name} locals) into <file>
  --line-table <file>  write the address-to-source-line table into <file>
                       (one `file:line:col <addr>` record per word)
  -O0 | -O1 | -O2 | -Os  optimization profile (default: -Os, today's
                       size-first pipeline byte for byte; -O0 folds
                       constants only, for stepping and bisection; -O2
                       trades flash for speed: no factoring, folding
                       into main/ISR within budget)
  -v                   echo the clang and llvm-link commands
  --no-outline         PIC18: keep repeated code inline instead of sharing it
                       (smaller flash by default; opt out for timing-critical
                       code or stepping through inline copies)
  --const-pool         PIC14: also emit address-taken const bytes as one
                       pooled flash table alongside the per-const tables,
                       rewrite their addresses to it, and drop their RAM
                       copies (epic-cc#815, epic-cc#816)
  --report <file>      write the build report as JSON into <file>: flash and
                       RAM use, the clock, and every config field's value
  --f-cpu <hz>         board clock in Hz, the last-resort D-4 source: it must
                       agree with the config/_XTAL_FREQ clock when they also fix one
  --var-table <file>   write the typed variable table into <file>
                       (`global <name> 0xNN TYPE` / `local {func}::{name}
                       0xNN TYPE`, one flattened record per mapped var)
  --version, -V        print the compiler identity (e.g. epic-cc 0.3.0, or
                       epic-cc 0.1.0+<sha> for a build from a git checkout)
  --resolve-device     resolve the next argument to a canonical device name
                       and print it, no input files needed (same resolution
                       --target uses, e.g. 16F877A and PIC16F877A -> p16f877a)
  --print-include-dir  print the effective header dir (the `include/`
                       shipped beside the binary, else the materialized
                       fallback) for the PlatformIO builder's CPPPATH
  --dump-include-dir <dir>
                       write every shipped header into <dir> (release-bundle
                       tooling; the bundle carries exactly what the driver
                       would materialize)
";
/// Parse an argument list that does NOT include `argv[0]`.
pub fn parse_args(argv: &[String]) -> Result<Cli, String> {
    let mut inputs = Vec::new();
    let mut output = None;
    let mut includes = Vec::new();
    let mut defines = Vec::new();
    let mut device = None;
    let mut emit = Emit::Hex;
    let mut save_temps = None;
    let mut verbose = false;
    let mut map = None;
    let mut line_table = None;
    let mut var_table = None;
    let mut sidecar = None;
    let mut report = None;
    let mut f_cpu: Option<u64> = None;
    let mut outline = true;
    let mut opt_level = OptLevel::default();
    let mut const_pool = false;
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        // Short flags take their value attached (`-Iinc`) or separate (`-I inc`).
        if let Some(rest) = a.strip_prefix("-I") {
            if !rest.is_empty() {
                includes.push(rest.to_string());
            } else {
                i += 1;
                includes.push(argv.get(i).cloned().ok_or("epic-cc: -I needs a value")?);
            }
        } else if let Some(rest) = a.strip_prefix("-D") {
            if !rest.is_empty() {
                defines.push(rest.to_string());
            } else {
                i += 1;
                defines.push(argv.get(i).cloned().ok_or("epic-cc: -D needs a value")?);
            }
        } else if let Some(rest) = a.strip_prefix("-o") {
            if !rest.is_empty() {
                output = Some(rest.to_string());
            } else {
                i += 1;
                output = Some(argv.get(i).cloned().ok_or("epic-cc: -o needs a value")?);
            }
        } else if a == "--device" || a == "--target" || a == "--mcu" || a == "-mcu" {
            i += 1;
            device = Some(
                argv.get(i)
                    .cloned()
                    .ok_or(format!("epic-cc: {a} needs a value"))?,
            );
        } else if a == "--emit" {
            i += 1;
            let v = argv
                .get(i)
                .cloned()
                .ok_or("epic-cc: --emit needs a value")?;
            emit = match v.as_str() {
                "ll" => Emit::Ll,
                "ir" => Emit::Ir,
                "asm" => Emit::Asm,
                "hex" => Emit::Hex,
                other => return Err(format!("epic-cc: unknown --emit stage {other}")),
            };
        } else if a == "--save-temps" {
            i += 1;
            save_temps = Some(
                argv.get(i)
                    .cloned()
                    .ok_or("epic-cc: --save-temps needs a value")?,
            );
        } else if a == "--report" {
            i += 1;
            report = Some(
                argv.get(i)
                    .cloned()
                    .ok_or("epic-cc: --report needs a value")?,
            );
        } else if a == "--f-cpu" {
            i += 1;
            let v = argv
                .get(i)
                .cloned()
                .ok_or("epic-cc: --f-cpu needs a value")?;
            let hz: u64 = v.replace('_', "").parse().map_err(|_| {
                format!("epic-cc: --f-cpu value {v:?} is not an integer frequency in Hz")
            })?;
            if hz == 0 {
                return Err(format!(
                    "epic-cc: --f-cpu value {v:?} is not a positive frequency"
                ));
            }
            f_cpu = Some(hz);
        } else if a == "--map" {
            i += 1;
            map = Some(argv.get(i).cloned().ok_or("epic-cc: --map needs a value")?);
        } else if a == "--line-table" {
            i += 1;
            line_table = Some(
                argv.get(i)
                    .cloned()
                    .ok_or("epic-cc: --line-table needs a value")?,
            );
        } else if matches!(a, "-O0" | "-O1" | "-O2" | "-Os") {
            opt_level = match a {
                "-O0" => OptLevel::O0,
                "-O1" => OptLevel::O1,
                "-O2" => OptLevel::O2,
                _ => OptLevel::Os,
            };
        } else if a == "-v" {
            verbose = true;
        } else if a == "--no-outline" {
            outline = false;
        } else if a == "--const-pool" {
            const_pool = true;
        } else if a == "--sidecar" {
            i += 1;
            sidecar = Some(
                argv.get(i)
                    .cloned()
                    .ok_or("epic-cc: --sidecar needs a value")?,
            );
        } else if a == "--var-table" {
            i += 1;
            var_table = Some(
                argv.get(i)
                    .cloned()
                    .ok_or("epic-cc: --var-table needs a value")?,
            );
        } else if a.starts_with('-') {
            return Err(format!("epic-cc: unknown option {a}\n\n{USAGE}"));
        } else {
            inputs.push(a.to_string());
        }
        i += 1;
    }
    if inputs.is_empty() {
        return Err(format!("epic-cc: no input files\n\n{USAGE}"));
    }
    let device = device.ok_or_else(|| format!("epic-cc: --target is required\n\n{USAGE}"))?;

    Ok(Cli {
        inputs,
        output: output.unwrap_or_else(|| "a.hex".to_string()),
        includes,
        defines,
        device,
        emit,
        save_temps,
        var_table,
        sidecar,
        report,
        f_cpu,
        verbose,
        map,
        line_table,
        opt_level,
        outline,
        const_pool,
    })
}
