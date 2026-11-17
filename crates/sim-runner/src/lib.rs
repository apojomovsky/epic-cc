//! `epic-cc-sim`: batch simulator runner over `pic14-sim` (design 51).
//! Takes a compiled HEX file, steps it to a halt or the cycle cap, and
//! prints one final state line; diagnostics go to stderr.

use device::Core;
use pic14_sim::{Pic14, Pic14e, Pic18, PicBaseline};

/// Parsed `epic-cc-sim <hex> [--device <name>] [--cycles <n>]` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub hex_path: String,
    pub device: Option<String>,
    pub cycles: u64,
}

/// The usage contract `--help` prints and failures append.
pub const USAGE: &str = "usage: epic-cc-sim <hex> [--device <name>] [--cycles <n>]";

/// Default cycle cap from the design 51 CLI contract.
pub const DEFAULT_CYCLES: u64 = 1_000_000;

/// Default device from the design 51 CLI contract.
pub const DEFAULT_DEVICE: &str = "p16f877a";

const KNOWN_FLAGS: &[&str] = &["--device", "--cycles", "--help", "--version"];
const SUGGEST_MAX_DISTANCE: usize = 3;

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = (diag + usize::from(ca != cb)).min(row[j] + 1).min(up + 1);
            diag = up;
        }
    }
    row[b.len()]
}

fn suggest(got: &str, candidates: &[&str]) -> String {
    candidates
        .iter()
        .map(|c| (edit_distance(got, c), *c))
        .filter(|(d, _)| *d <= SUGGEST_MAX_DISTANCE)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| format!(" (did you mean {c}?)"))
        .unwrap_or_default()
}

fn usage_err(msg: String) -> String {
    format!("{msg}\n\n{USAGE}")
}

/// Parse a plain decimal or `0x` hex count, the `--cycles` value shape.
pub fn parse_count(text: &str) -> Result<u64, String> {
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse::<u64>(),
    };
    parsed.map_err(|_| format!("bad number {text}"))
}

/// Parse the runner command line. Errors are plain strings carrying the
/// usage contract, so `main` can print them and exit 2 unchanged.
pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut hex_path: Option<String> = None;
    let mut device: Option<String> = None;
    let mut cycles = DEFAULT_CYCLES;
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        i += 1;
        let (flag, inline): (&str, Option<String>) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        match flag {
            "--device" | "--cycles" => {
                let value = match inline {
                    Some(v) => v,
                    None => {
                        let v = argv
                            .get(i)
                            .ok_or_else(|| usage_err(format!("{flag} needs a value")))?;
                        i += 1;
                        v.clone()
                    }
                };
                if flag == "--device" {
                    device = Some(value);
                } else {
                    cycles =
                        parse_count(&value).map_err(|e| usage_err(format!("--cycles: {e}")))?;
                }
            }
            _ if flag.starts_with("--") => {
                let hint = suggest(flag, KNOWN_FLAGS);
                return Err(usage_err(format!(
                    "epic-cc-sim: unknown option {flag}{hint}"
                )));
            }
            _ => {
                if hex_path.is_some() {
                    return Err(usage_err(format!(
                        "epic-cc-sim: unexpected extra argument {a}"
                    )));
                }
                hex_path = Some(a.clone());
            }
        }
    }
    let hex_path = hex_path.ok_or_else(|| usage_err("epic-cc-sim: missing <hex>".to_string()))?;
    Ok(Args {
        hex_path,
        device,
        cycles,
    })
}

/// Resolve the `--device` spelling (or the default) to its registry entry.
pub fn resolve_device(name: Option<&str>) -> Result<&'static device::Device, String> {
    let name = name.unwrap_or(DEFAULT_DEVICE);
    device::resolve(name).ok_or_else(|| {
        let available = device::ALL
            .iter()
            .map(|d| d.name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("epic-cc-sim: unknown device {name} (available: {available})")
    })
}

/// Validate one HEX text against the design 51 checklist, returning the
/// 1-based line number of the first failing record.
pub fn validate_hex(text: &str) -> Result<(), usize> {
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let bad = || idx + 1;
        let Some(body) = line.strip_prefix(':') else {
            return Err(bad());
        };
        if body.len() % 2 != 0 || !body.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let bytes: Vec<u8> = (0..body.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&body[i..i + 2], 16).unwrap_or(0))
            .collect();
        if bytes.len() < 5 {
            return Err(bad());
        }
        let count = bytes[0] as usize;
        let rectype = bytes[3];
        if bytes.len() != count + 5 {
            return Err(bad());
        }
        if bytes.iter().map(|&b| b as u32).sum::<u32>() % 256 != 0 {
            return Err(bad());
        }
        if !matches!(rectype, 0x00 | 0x01 | 0x04) {
            return Err(bad());
        }
        if rectype == 0x00 && count % 2 != 0 {
            return Err(bad());
        }
    }
    Ok(())
}

fn parse_program(text: &str, core: Core) -> Vec<u16> {
    match core {
        Core::Pic14 | Core::PicBaseline => pic14_sim::parse_hex(text),
        Core::Pic14e => pic14_sim::parse_hex_pic14e(text),
        Core::Pic18 => pic14_sim::parse_hex_pic18(text),
    }
}

/// Load and parse a HEX file, mapping every failure to an exit-1 string.
/// The parser boundary is `catch_unwind` with a silenced hook: the sim
/// parsers panic on malformed input, and no panic may reach the user.
pub fn load_program(path: &str, core: Core) -> Result<Vec<u16>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("epic-cc-sim: read {path}: {e}"))?;
    if let Err(line) = validate_hex(&text) {
        return Err(format!("malformed HEX in {path}:{line}"));
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse_program(&text, core)));
    std::panic::set_hook(previous);
    result.map_err(|_| format!("malformed HEX in {path}"))
}

/// The four sim cores behind one dispatch, per design 51 D-3.
pub enum Sim {
    Pic14(Pic14),
    Pic14e(Pic14e),
    PicBaseline(PicBaseline<'static>),
    Pic18(Pic18),
}

impl Sim {
    /// Build the core `device` selects; Pic18 has no device constructor,
    /// so its device picks only the parser and names (design 51 D-3).
    pub fn new(device: &'static device::Device, prog: Vec<u16>) -> Self {
        match device.core {
            Core::Pic14 => Sim::Pic14(Pic14::with_device(device, prog)),
            Core::Pic14e => Sim::Pic14e(Pic14e::with_device(device, prog)),
            Core::PicBaseline => Sim::PicBaseline(PicBaseline::with_device(device, prog)),
            Core::Pic18 => Sim::Pic18(Pic18::new(prog)),
        }
    }

    pub fn step(&mut self) {
        match self {
            Sim::Pic14(s) => s.step(),
            Sim::Pic14e(s) => s.step(),
            Sim::PicBaseline(s) => s.step(),
            Sim::Pic18(s) => s.step(),
        }
    }

    pub fn halted(&self) -> bool {
        match self {
            Sim::Pic14(s) => s.halted(),
            Sim::Pic14e(s) => s.halted(),
            Sim::PicBaseline(s) => s.halted(),
            Sim::Pic18(s) => s.halted(),
        }
    }

    pub fn cycles(&self) -> u64 {
        match self {
            Sim::Pic14(s) => s.cycles(),
            Sim::Pic14e(s) => s.cycles(),
            Sim::PicBaseline(s) => s.cycles(),
            Sim::Pic18(s) => s.cycles(),
        }
    }

    /// Program counter as one integer; Pic18 counts bytes, the rest words.
    pub fn pc(&self) -> u32 {
        match self {
            Sim::Pic14(s) => s.pc() as u32,
            Sim::Pic14e(s) => s.pc() as u32,
            Sim::PicBaseline(s) => s.pc() as u32,
            Sim::Pic18(s) => s.pc(),
        }
    }

    pub fn w(&self) -> u8 {
        match self {
            Sim::Pic14(s) => s.w(),
            Sim::Pic14e(s) => s.w(),
            Sim::PicBaseline(s) => s.w(),
            Sim::Pic18(s) => s.w(),
        }
    }

    pub fn status(&self) -> u8 {
        match self {
            Sim::Pic14(s) => s.status(),
            Sim::Pic14e(s) => s.status(),
            Sim::PicBaseline(s) => s.status(),
            Sim::Pic18(s) => s.status(),
        }
    }

    pub fn ram(&self) -> &[u8] {
        match self {
            Sim::Pic14(s) => s.ram(),
            Sim::Pic14e(s) => s.ram(),
            Sim::PicBaseline(s) => s.ram(),
            Sim::Pic18(s) => s.ram(),
        }
    }

    /// EEPROM image where the core has one; baseline parts have none.
    pub fn eeprom(&self) -> Option<&[u8; 256]> {
        match self {
            Sim::Pic14(s) => Some(s.eeprom()),
            Sim::Pic14e(s) => Some(s.eeprom()),
            Sim::PicBaseline(_) => None,
            Sim::Pic18(s) => Some(s.eeprom()),
        }
    }

    /// Hex digits in a formatted `pc=`: 6 on Pic18, 4 on the other cores.
    pub fn pc_width(&self) -> usize {
        match self {
            Sim::Pic18(_) => 6,
            _ => 4,
        }
    }
}

/// Why the run loop stopped; exit codes 0 and 3 in T1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Halted,
    Capped,
}

impl Stop {
    pub fn word(&self) -> &'static str {
        match self {
            Stop::Halted => "halted",
            Stop::Capped => "capped",
        }
    }
    pub fn exit_code(&self) -> i32 {
        match self {
            Stop::Halted => 0,
            Stop::Capped => 3,
        }
    }
}

/// The final state of a run, formatted as the one final stdout line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub stop: Stop,
    pub cycles: u64,
    pub pc: u32,
    pub pc_width: usize,
    pub w: u8,
    pub status: u8,
}

impl Outcome {
    pub fn final_line(&self) -> String {
        format!(
            "{} after {} cycles pc=0x{:0width$x} w=0x{:02x} status=0x{:02x}",
            self.stop.word(),
            self.cycles,
            self.pc,
            self.w,
            self.status,
            width = self.pc_width
        )
    }
}

/// Step one instruction at a time to a halt or the cycle cap (design 51
/// D-5: the loop never calls the bulk `run`/`run_until` primitives).
pub fn run(sim: &mut Sim, cap: u64) -> Outcome {
    while !sim.halted() && sim.cycles() < cap {
        sim.step();
    }
    let stop = if sim.halted() {
        Stop::Halted
    } else {
        Stop::Capped
    };
    Outcome {
        stop,
        cycles: sim.cycles(),
        pc: sim.pc(),
        pc_width: sim.pc_width(),
        w: sim.w(),
        status: sim.status(),
    }
}

/// Run the whole invocation after `--help`/`--version` handling, returning
/// the process exit code; the final line is the only stdout output.
pub fn run_main(args: &Args) -> i32 {
    let device = match resolve_device(args.device.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let prog = match load_program(&args.hex_path, device.core) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut sim = Sim::new(device, prog);
    let outcome = run(&mut sim, args.cycles);
    println!("{}", outcome.final_line());
    outcome.stop.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_apply_without_flags() {
        let a = parse_args(&argv(&["blink.hex"])).unwrap();
        assert_eq!(a.hex_path, "blink.hex");
        assert_eq!(a.device, None);
        assert_eq!(a.cycles, DEFAULT_CYCLES);
    }

    #[test]
    fn device_and_cycles_accept_separate_and_inline_values() {
        let a = parse_args(&argv(&["a.hex", "--device", "16F877A", "--cycles=0x10"])).unwrap();
        assert_eq!(a.device.as_deref(), Some("16F877A"));
        assert_eq!(a.cycles, 16);
        let b = parse_args(&argv(&["--cycles", "42", "--device=p16f877a", "a.hex"])).unwrap();
        assert_eq!(b.cycles, 42);
        assert_eq!(b.device.as_deref(), Some("p16f877a"));
    }

    #[test]
    fn missing_hex_is_a_usage_error() {
        let e = parse_args(&argv(&[])).unwrap_err();
        assert!(e.contains("missing <hex>"), "{e}");
        assert!(e.contains(USAGE), "{e}");
    }

    #[test]
    fn unknown_flag_suggests_the_nearest_known_flag() {
        let e = parse_args(&argv(&["a.hex", "--devce", "x"])).unwrap_err();
        assert!(e.contains("did you mean --device?"), "{e}");
        assert!(e.contains(USAGE), "{e}");
    }

    #[test]
    fn bad_cycles_and_missing_values_are_usage_errors() {
        assert!(parse_args(&argv(&["a.hex", "--cycles", "soon"])).is_err());
        assert!(parse_args(&argv(&["a.hex", "--cycles"])).is_err());
        assert!(parse_args(&argv(&["a.hex", "--device"])).is_err());
    }

    #[test]
    fn extra_positional_is_a_usage_error() {
        assert!(parse_args(&argv(&["a.hex", "b.hex"])).is_err());
    }

    #[test]
    fn parse_count_accepts_decimal_and_hex_only() {
        assert_eq!(parse_count("1000000").unwrap(), 1_000_000);
        assert_eq!(parse_count("0xff").unwrap(), 255);
        assert!(parse_count("-1").is_err());
        assert!(parse_count("1.5").is_err());
    }

    #[test]
    fn resolve_device_defaults_and_rejects_unknown() {
        assert_eq!(resolve_device(None).unwrap().core, Core::Pic14);
        assert_eq!(
            resolve_device(Some("PIC16F877A")).unwrap().name,
            resolve_device(None).unwrap().name
        );
        assert!(resolve_device(Some("no-such-part")).is_err());
    }

    #[test]
    fn validate_hex_accepts_a_well_formed_record_set() {
        assert!(validate_hex(":040000000230630067\n:00000001FF\n").is_ok());
        assert!(validate_hex(":020000040000FA\n:00000001FF\n").is_ok());
    }

    #[test]
    fn validate_hex_names_the_first_bad_line() {
        // Bad checksum on line 2.
        assert_eq!(validate_hex(":040000000230630067\n:00000001FE\n"), Err(2));
        // Missing colon, odd digits, short record, bad type, odd data.
        assert_eq!(validate_hex("00000001FF\n"), Err(1));
        assert_eq!(validate_hex(":0000001FF\n"), Err(1));
        assert_eq!(validate_hex(":0001FF\n"), Err(1));
        assert_eq!(validate_hex(":020000020000FA\n"), Err(1));
        assert_eq!(validate_hex(":0100000000FF\n"), Err(1));
    }
}
