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
}

pub const USAGE: &str = "\
usage: epic-cc [options] <input.c>...

  -o <file>            output file (default: a.hex)
  -I <dir>             include path, repeatable, forwarded to clang
  -D <name[=value]>    define, repeatable, forwarded to clang
  --target <name>      device name (e.g. p16f877a, p18f4550); aliases: --device, --mcu, -mcu
  --emit <stage>       ll | ir | asm | hex (default: hex)
  --save-temps <dir>   write every stage artifact into <dir>
  -v                   echo the clang and llvm-link commands
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
        } else if a == "-v" {
            verbose = true;
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
        verbose,
    })
}
