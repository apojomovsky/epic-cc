//! `epic-cc-sim`: batch simulator runner over `pic14-sim` (design 51).
//! Takes a compiled HEX file, steps it to a halt or the cycle cap, and
//! prints one final state line; diagnostics go to stderr.

use device::Core;
use pic14_sim::{Pic14, Pic14e, Pic18, PicBaseline};

/// Parsed `epic-cc-sim` arguments (design 51 section 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub hex_path: String,
    pub device: Option<String>,
    pub cycles: u64,
    pub breaks: Vec<u32>,
    pub pokes: Vec<(u32, u8)>,
    pub stimulus: Option<String>,
    pub trace_sfr: Vec<String>,
    pub dump: Option<DumpKind>,
}

/// What `--dump` prints before the final line (design 51 section 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpKind {
    Regs,
    Ram,
    All,
}

impl DumpKind {
    fn parse(text: &str) -> Result<Self, String> {
        match text {
            "regs" => Ok(DumpKind::Regs),
            "ram" => Ok(DumpKind::Ram),
            "all" => Ok(DumpKind::All),
            _ => Err(format!(
                "bad --dump value {text} (expected regs, ram, or all)"
            )),
        }
    }
}

/// The usage contract `--help` prints and failures append.
pub const USAGE: &str = "usage: epic-cc-sim <hex> [--device <name>] [--cycles <n>] [--break <addr>]... [--poke <addr>=<byte>]... [--stimulus <file>] [--trace-sfr <name|addr>[,...]] [--dump regs|ram|all]";

/// Default cycle cap from the design 51 CLI contract.
pub const DEFAULT_CYCLES: u64 = 1_000_000;

/// Default device from the design 51 CLI contract.
pub const DEFAULT_DEVICE: &str = "p16f877a";

const KNOWN_FLAGS: &[&str] = &[
    "--device",
    "--cycles",
    "--break",
    "--poke",
    "--stimulus",
    "--trace-sfr",
    "--dump",
    "--help",
    "--version",
];
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

fn parse_byte(text: &str) -> Result<u8, String> {
    let v = parse_count(text)?;
    u8::try_from(v).map_err(|_| format!("bad byte {text}"))
}

fn parse_poke(text: &str) -> Result<(u32, u8), String> {
    let (addr, byte) = text
        .split_once('=')
        .ok_or_else(|| format!("bad poke {text}"))?;
    let addr = u32::try_from(parse_count(addr)?).map_err(|_| format!("bad poke {text}"))?;
    Ok((
        addr,
        parse_byte(byte).map_err(|_| format!("bad poke {text}"))?,
    ))
}

/// Parse the runner command line. Errors are plain strings carrying the
/// usage contract, so `main` can print them and exit 2 unchanged.
pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut hex_path: Option<String> = None;
    let mut device: Option<String> = None;
    let mut cycles = DEFAULT_CYCLES;
    let mut breaks: Vec<u32> = Vec::new();
    let mut pokes: Vec<(u32, u8)> = Vec::new();
    let mut stimulus: Option<String> = None;
    let mut trace_sfr: Vec<String> = Vec::new();
    let mut dump: Option<DumpKind> = None;
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        i += 1;
        let (flag, inline): (&str, Option<String>) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        match flag {
            "--device" | "--cycles" | "--break" | "--poke" | "--stimulus" | "--trace-sfr"
            | "--dump" => {
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
                match flag {
                    "--device" => device = Some(value),
                    "--cycles" => {
                        cycles =
                            parse_count(&value).map_err(|e| usage_err(format!("--cycles: {e}")))?;
                    }
                    "--break" => {
                        let addr =
                            parse_count(&value).map_err(|e| usage_err(format!("--break: {e}")))?;
                        breaks.push(
                            u32::try_from(addr)
                                .map_err(|_| usage_err(format!("--break: bad address {value}")))?,
                        );
                    }
                    "--poke" => {
                        pokes.push(
                            parse_poke(&value).map_err(|e| usage_err(format!("--poke: {e}")))?,
                        );
                    }
                    "--stimulus" => stimulus = Some(value),
                    "--trace-sfr" => {
                        for item in value.split(',') {
                            let item = item.trim();
                            if item.is_empty() {
                                return Err(usage_err(
                                    "epic-cc-sim: empty entry in --trace-sfr".to_string(),
                                ));
                            }
                            trace_sfr.push(item.to_string());
                        }
                    }
                    _ => {
                        dump = Some(DumpKind::parse(&value).map_err(usage_err)?);
                    }
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
        breaks,
        pokes,
        stimulus,
        trace_sfr,
        dump,
    })
}

/// Physical `ram()` size in bytes for `core` (design 51 D-5).
pub fn ram_size(core: Core) -> u32 {
    match core {
        Core::PicBaseline => 256,
        Core::Pic14 => 512,
        Core::Pic14e | Core::Pic18 => 4096,
    }
}

/// Addresses a poke must not target: the `INDF` indirect cell and the
/// pseudo-registers whose live value is not the `ram()` cell (design 51 D-5).
pub fn is_poke_forbidden(core: Core, addr: u32) -> bool {
    match core {
        Core::Pic14 | Core::PicBaseline => matches!(addr, 0x00 | 0x02),
        Core::Pic14e => matches!(addr, 0x00 | 0x01 | 0x02 | 0x09),
        Core::Pic18 => matches!(addr, 0xFEF | 0xFE7 | 0xFDF | 0xFE8 | 0xFF9),
    }
}

fn poke_addr_problem(core: Core, addr: u32) -> Option<String> {
    if addr >= ram_size(core) {
        Some(format!(
            "poke address 0x{addr:x} out of range (ram size 0x{:x})",
            ram_size(core)
        ))
    } else if is_poke_forbidden(core, addr) {
        Some(format!(
            "poke address 0x{addr:x} is a pseudo-register or INDF address"
        ))
    } else {
        None
    }
}

/// Validate a flag `--poke` address for `core`; failure is a usage error.
pub fn validate_flag_poke(core: Core, addr: u32) -> Result<(), String> {
    poke_addr_problem(core, addr)
        .map(|p| usage_err(format!("epic-cc-sim: --poke: {p}")))
        .map_or(Ok(()), Err)
}

/// One timed stimulus action (design 51 D-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StimulusAction {
    Poke(u32, u8),
    Interrupt,
}

/// A validated stimulus entry, applied when the cycle count reaches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StimulusEntry {
    pub cycle: u64,
    pub action: StimulusAction,
}

/// A stimulus failure: message already names file and line, plus exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StimulusError {
    pub message: String,
    pub exit_code: i32,
}

/// Parse and validate a whole stimulus file up front (design 51 D-5).
/// Syntax and poke problems exit 1; `interrupt` on a baseline device exits 2.
pub fn parse_stimulus(path: &str, core: Core) -> Result<Vec<StimulusEntry>, StimulusError> {
    let text = std::fs::read_to_string(path).map_err(|e| StimulusError {
        message: format!("epic-cc-sim: read {path}: {e}"),
        exit_code: 1,
    })?;
    let mut entries = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let err1 = |msg: String| StimulusError {
            message: format!("epic-cc-sim: {path}:{line_no}: {msg}"),
            exit_code: 1,
        };
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 || parts[0] != "at" {
            return Err(err1(format!("malformed stimulus line: {line}")));
        }
        let cycle =
            parse_count(parts[1]).map_err(|_| err1(format!("malformed stimulus line: {line}")))?;
        match parts[2] {
            "poke" => {
                if parts.len() != 5 {
                    return Err(err1(format!("malformed stimulus line: {line}")));
                }
                let addr = parse_count(parts[3])
                    .ok()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| err1(format!("malformed stimulus line: {line}")))?;
                let byte = parse_byte(parts[4])
                    .map_err(|_| err1(format!("malformed stimulus line: {line}")))?;
                if let Some(p) = poke_addr_problem(core, addr) {
                    return Err(err1(p));
                }
                entries.push(StimulusEntry {
                    cycle,
                    action: StimulusAction::Poke(addr, byte),
                });
            }
            "interrupt" => {
                if parts.len() != 3 {
                    return Err(err1(format!("malformed stimulus line: {line}")));
                }
                if core == Core::PicBaseline {
                    return Err(StimulusError {
                        message: usage_err(format!(
                            "epic-cc-sim: {path}:{line_no}: interrupt is not available on a baseline device"
                        )),
                        exit_code: 2,
                    });
                }
                entries.push(StimulusEntry {
                    cycle,
                    action: StimulusAction::Interrupt,
                });
            }
            _ => return Err(err1(format!("malformed stimulus line: {line}"))),
        }
    }
    Ok(entries)
}

/// Resolve the `--device` spelling (or the default) to its registry entry.
/// An unknown name is a usage error, so the error carries the usage text.
pub fn resolve_device(name: Option<&str>) -> Result<&'static device::Device, String> {
    let name = name.unwrap_or(DEFAULT_DEVICE);
    device::resolve(name).ok_or_else(|| {
        let available = device::ALL
            .iter()
            .map(|d| d.name)
            .collect::<Vec<_>>()
            .join(", ");
        usage_err(format!(
            "epic-cc-sim: unknown device {name} (available: {available})"
        ))
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

    /// Write one validated physical cell directly (design 51 D-5).
    pub fn poke(&mut self, addr: u32, byte: u8) {
        match self {
            Sim::Pic14(s) => s.ram_mut()[addr as usize] = byte,
            Sim::Pic14e(s) => s.ram_mut()[addr as usize] = byte,
            Sim::PicBaseline(s) => s.ram_mut()[addr as usize] = byte,
            Sim::Pic18(s) => s.ram_mut()[addr as usize] = byte,
        }
    }

    /// Request an interrupt through the gated hook; baseline has none.
    pub fn request_interrupt(&mut self) {
        match self {
            Sim::Pic14(s) => s.request_interrupt(),
            Sim::Pic14e(s) => s.request_interrupt(),
            Sim::PicBaseline(_) => {}
            Sim::Pic18(s) => s.request_interrupt(),
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

/// How a traced register's value is snapshotted (design 51 D-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceKind {
    /// The `width` bytes at `addr` in `ram()`, little-endian.
    Ram,
    /// `PCL`: snapshots `pc() & 0xFF`, not the dead `ram()` cell.
    Pcl,
    /// `WREG`: snapshots `w()`, not the dead `ram()` cell.
    Wreg,
}

/// One resolved `--trace-sfr` entry, deduplicated on `(addr, width)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Traced {
    pub display: String,
    pub addr: u16,
    pub width: u8,
    pub kind: TraceKind,
}

impl Traced {
    fn snapshot(&self, sim: &Sim) -> u64 {
        match self.kind {
            TraceKind::Pcl => (sim.pc() & 0xFF) as u64,
            TraceKind::Wreg => sim.w() as u64,
            TraceKind::Ram => {
                let start = self.addr as usize;
                let mut value: u64 = 0;
                for (i, &b) in sim.ram()[start..start + self.width as usize]
                    .iter()
                    .enumerate()
                {
                    value |= (b as u64) << (8 * i);
                }
                value
            }
        }
    }

    fn line(&self, cycles: u64, pre_pc: u32, pc_width: usize, value: u64) -> String {
        format!(
            "cycle={cycles} pc=0x{:0pw$x} write {}=0x{:0vw$x}",
            pre_pc,
            self.display,
            value,
            pw = pc_width,
            vw = 2 * self.width as usize
        )
    }
}

// The SFR table names each core's indirect registers (Pic14e has both
// INDF0 and INDF1, Pic18 has three), so derive the set from it and add
// the core's architectural addresses for devices with no SFR table.
fn indf_addrs(device: &device::Device) -> Vec<u16> {
    let mut addrs: Vec<u16> = device
        .sfrs
        .iter()
        .filter(|s| is_indf_name(s.name) || s.aliases.iter().any(|a| is_indf_name(a)))
        .map(|s| s.addr)
        .collect();
    addrs.extend(match device.core {
        Core::Pic18 => &[0xFEF, 0xFE7, 0xFDF][..],
        Core::Pic14e => &[0x00, 0x01][..],
        _ => &[0x00][..],
    });
    addrs.sort_unstable();
    addrs.dedup();
    addrs
}

fn pseudo_kind(device: &device::Device, addr: u16) -> TraceKind {
    for sfr in device.sfrs.iter().filter(|s| s.addr == addr) {
        let named = |want: &str| {
            sfr.name.eq_ignore_ascii_case(want)
                || sfr.aliases.iter().any(|a| a.eq_ignore_ascii_case(want))
        };
        if named("PCL") {
            return TraceKind::Pcl;
        }
        if named("WREG") {
            return TraceKind::Wreg;
        }
    }
    match device.core {
        Core::Pic14e if addr == 0x09 => TraceKind::Wreg,
        Core::Pic18 if addr == 0xFE8 => TraceKind::Wreg,
        _ if addr == 0x02 && device.core != Core::Pic18 => TraceKind::Pcl,
        Core::Pic18 if addr == 0xFF9 => TraceKind::Pcl,
        _ => TraceKind::Ram,
    }
}

fn parse_address(text: &str) -> Option<u64> {
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        if !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return u64::from_str_radix(hex, 16).ok();
        }
        return None;
    }
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return text.parse::<u64>().ok();
    }
    None
}

fn is_indf_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("INDF") || name.to_ascii_uppercase().starts_with("INDF")
}

/// Resolve `--trace-sfr` items over `device.sfrs` (design 51 D-4 and
/// section 3): canonical display, first match by address, deduplication
/// on `(addr, width)`, and the pseudo-register and `INDF` rules. Every
/// failure is a usage error carrying the usage text (exit 2).
pub fn resolve_traces(
    device: &device::Device,
    ram_len: usize,
    items: &[String],
) -> Result<Vec<Traced>, String> {
    let core = device.core;
    let indf = indf_addrs(device);
    let mut traced: Vec<Traced> = Vec::new();
    let push = |t: Traced, traced: &mut Vec<Traced>| {
        if !traced
            .iter()
            .any(|e| e.addr == t.addr && e.width == t.width)
        {
            traced.push(t);
        }
    };
    for item in items {
        let resolved: Traced = if let Some(addr) = parse_address(item) {
            if addr > u16::MAX as u64 {
                return Err(usage_err(format!(
                    "epic-cc-sim: --trace-sfr address {item} out of range"
                )));
            }
            let addr = addr as u16;
            if indf.contains(&addr) {
                return Err(usage_err(
                    "epic-cc-sim: INDF is not traceable; trace the target register instead"
                        .to_string(),
                ));
            }
            if addr as usize >= ram_len {
                return Err(usage_err(format!(
                    "epic-cc-sim: --trace-sfr address {item} out of range"
                )));
            }
            let display = match device.sfrs.iter().find(|s| s.addr == addr) {
                Some(s) => s.name.to_string(),
                None => {
                    let digits = if core == Core::PicBaseline { 2 } else { 3 };
                    format!("0x{addr:0digits$x}")
                }
            };
            Traced {
                display,
                addr,
                width: 1,
                kind: pseudo_kind(device, addr),
            }
        } else {
            let Some(sfr) = device.sfrs.iter().find(|s| {
                s.name.eq_ignore_ascii_case(item)
                    || s.aliases.iter().any(|a| a.eq_ignore_ascii_case(item))
            }) else {
                return Err(usage_err(format!(
                    "epic-cc-sim: unknown SFR name {item} in --trace-sfr"
                )));
            };
            if is_indf_name(sfr.name) || indf.contains(&sfr.addr) {
                return Err(usage_err(
                    "epic-cc-sim: INDF is not traceable; trace the target register instead"
                        .to_string(),
                ));
            }
            if sfr.addr as usize + sfr.width as usize > ram_len {
                return Err(usage_err(format!(
                    "epic-cc-sim: --trace-sfr {} out of range",
                    sfr.name
                )));
            }
            Traced {
                display: sfr.name.to_string(),
                addr: sfr.addr,
                width: sfr.width,
                kind: pseudo_kind(device, sfr.addr),
            }
        };
        push(resolved, &mut traced);
    }
    Ok(traced)
}

/// Dump lines in the design 51 byte-stable order, before the final line.
pub fn dump_lines(sim: &Sim, kind: DumpKind) -> Vec<String> {
    let mut lines = vec![
        format!("w=0x{:02x}", sim.w()),
        format!("status=0x{:02x}", sim.status()),
        format!("pc=0x{:0width$x}", sim.pc(), width = sim.pc_width()),
    ];
    if kind != DumpKind::Regs {
        for (base, block) in sim.ram().chunks(16).enumerate() {
            if kind == DumpKind::All || block.iter().any(|&b| b != 0) {
                let bytes: Vec<String> = block.iter().map(|b| format!("{b:02x}")).collect();
                lines.push(format!("ram 0x{:04x}: {}", base * 16, bytes.join(" ")));
            }
        }
    }
    if kind == DumpKind::All {
        if let Some(eeprom) = sim.eeprom() {
            for (base, block) in eeprom.chunks(16).enumerate() {
                let bytes: Vec<String> = block.iter().map(|b| format!("{b:02x}")).collect();
                lines.push(format!("eeprom 0x{:04x}: {}", base * 16, bytes.join(" ")));
            }
        }
    }
    lines
}

/// Why the run loop stopped; exit codes 0 and 3 in T1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Halted,
    Capped,
    Breakpoint,
}

impl Stop {
    pub fn word(&self) -> &'static str {
        match self {
            Stop::Halted => "halted",
            Stop::Capped => "capped",
            Stop::Breakpoint => "breakpoint",
        }
    }
    pub fn exit_code(&self) -> i32 {
        match self {
            Stop::Halted => 0,
            Stop::Capped => 3,
            Stop::Breakpoint => 4,
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

/// Step one instruction at a time (design 51 D-5 run-loop order): apply
/// due stimulus, check breakpoints on the pre-step pc, snapshot traced
/// registers, step, emit trace lines for changed registers, then check
/// halt and the cycle cap.
pub fn run(sim: &mut Sim, cap: u64, breaks: &[u32], stimulus: &[StimulusEntry]) -> Outcome {
    run_loop(sim, cap, breaks, stimulus, &[]).0
}

/// The run loop with the T3 step hook: after each step, emit one trace
/// line per traced register whose snapshot changed (change-only, D-4).
pub fn run_traced(sim: &mut Sim, cap: u64, traced: &[Traced]) -> (Outcome, Vec<String>) {
    run_loop(sim, cap, &[], &[], traced)
}

fn run_loop(
    sim: &mut Sim,
    cap: u64,
    breaks: &[u32],
    stimulus: &[StimulusEntry],
    traced: &[Traced],
) -> (Outcome, Vec<String>) {
    let mut lines = Vec::new();
    let mut pending: Vec<StimulusEntry> = stimulus.to_vec();
    let stop = loop {
        let now = sim.cycles();
        let mut rest = Vec::new();
        for e in pending {
            if e.cycle <= now {
                match e.action {
                    StimulusAction::Poke(a, b) => sim.poke(a, b),
                    StimulusAction::Interrupt => sim.request_interrupt(),
                }
            } else {
                rest.push(e);
            }
        }
        pending = rest;
        if breaks.contains(&sim.pc()) {
            break Stop::Breakpoint;
        }
        if sim.halted() {
            break Stop::Halted;
        }
        if sim.cycles() >= cap {
            break Stop::Capped;
        }
        let pre_pc = sim.pc();
        let before: Vec<u64> = traced.iter().map(|t| t.snapshot(sim)).collect();
        sim.step();
        for (t, &b) in traced.iter().zip(&before) {
            let after = t.snapshot(sim);
            if after != b {
                lines.push(t.line(sim.cycles(), pre_pc, sim.pc_width(), after));
            }
        }
        if sim.halted() {
            break Stop::Halted;
        }
        if sim.cycles() >= cap {
            break Stop::Capped;
        }
    };
    let outcome = Outcome {
        stop,
        cycles: sim.cycles(),
        pc: sim.pc(),
        pc_width: sim.pc_width(),
        w: sim.w(),
        status: sim.status(),
    };
    (outcome, lines)
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
    for &(addr, _) in &args.pokes {
        if let Err(e) = validate_flag_poke(device.core, addr) {
            eprintln!("{e}");
            return 2;
        }
    }
    let stimulus = match &args.stimulus {
        Some(path) => match parse_stimulus(path, device.core) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}", e.message);
                return e.exit_code;
            }
        },
        None => Vec::new(),
    };
    let prog = match load_program(&args.hex_path, device.core) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut sim = Sim::new(device, prog);
    let traced = match resolve_traces(device, sim.ram().len(), &args.trace_sfr) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    for &(addr, byte) in &args.pokes {
        sim.poke(addr, byte);
    }
    let (outcome, trace_lines) = run_loop(&mut sim, args.cycles, &args.breaks, &stimulus, &traced);
    for line in &trace_lines {
        println!("{line}");
    }
    if let Some(kind) = args.dump {
        for line in dump_lines(&sim, kind) {
            println!("{line}");
        }
    }
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
        for parts in [
            &["a.hex", "--cycles", "soon"][..],
            &["a.hex", "--cycles"][..],
            &["a.hex", "--device"][..],
        ] {
            let e = parse_args(&argv(parts)).unwrap_err();
            assert!(e.contains(USAGE), "{e}");
        }
    }

    #[test]
    fn extra_positional_is_a_usage_error() {
        let e = parse_args(&argv(&["a.hex", "b.hex"])).unwrap_err();
        assert!(e.contains(USAGE), "{e}");
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
        let e = resolve_device(Some("no-such-part")).unwrap_err();
        assert!(e.contains("unknown device no-such-part"), "{e}");
        assert!(e.contains(USAGE), "{e}");
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

    #[test]
    fn trace_resolution_canonical_alias_and_first_match() {
        let dev = resolve_device(None).unwrap();
        let traced =
            resolve_traces(dev, 512, &["PORTB".into(), "0x06".into(), "portb".into()]).unwrap();
        assert_eq!(traced.len(), 1);
        assert_eq!(traced[0].display, "PORTB");
        assert_eq!((traced[0].addr, traced[0].width), (0x06, 1));
        // First entry at 0x0E is the joined TMR1 (width 2); a numeric
        // address still traces width 1 under that canonical name.
        let traced = resolve_traces(dev, 512, &["0x0e".into()]).unwrap();
        assert_eq!(traced[0].display, "TMR1");
        assert_eq!(traced[0].width, 1);
        let traced = resolve_traces(dev, 512, &["TMR1".into()]).unwrap();
        assert_eq!(traced[0].width, 2);
        // An address with no SFR entry displays as the padded address.
        let traced = resolve_traces(dev, 512, &["0x20".into()]).unwrap();
        assert_eq!(traced[0].display, "0x020");
    }

    #[test]
    fn trace_resolution_alias_uses_canonical_name() {
        let dev = resolve_device(Some("p18f4550")).unwrap();
        let traced = resolve_traces(dev, 4096, &["DDRA".into()]).unwrap();
        assert_eq!(traced[0].display, "TRISA");
        let traced = resolve_traces(dev, 4096, &["WREG".into()]).unwrap();
        assert_eq!(traced[0].kind, TraceKind::Wreg);
    }

    #[test]
    fn trace_resolution_rejects_unknown_indf_and_out_of_range() {
        let dev = resolve_device(None).unwrap();
        for items in [
            vec!["NOSUCHREG".to_string()],
            vec!["INDF".to_string()],
            vec!["0x00".to_string()],
            vec!["0x200".to_string()],
        ] {
            let e = resolve_traces(dev, 512, &items).unwrap_err();
            assert!(e.contains(USAGE), "{e}");
        }
        let e = resolve_traces(dev, 512, &["INDF".into()]).unwrap_err();
        assert!(e.contains("INDF is not traceable"), "{e}");
        // PCL is a pseudo-register traced through pc(), not ram().
        let traced = resolve_traces(dev, 512, &["PCL".into()]).unwrap();
        assert_eq!(traced[0].kind, TraceKind::Pcl);
    }

    #[test]
    fn indf_rejected_by_name_and_address_on_every_core() {
        // (device, ram length, INDF names in its table, INDF addresses)
        let cases: &[(&str, usize, &[&str], &[&str])] = &[
            ("p16f877a", 512, &["INDF"], &["0x00"]),
            ("p16f84a", 512, &[], &["0x00"]),
            ("p16f1937", 4096, &["INDF0", "INDF1"], &["0x00", "0x01"]),
            ("p16f1933", 4096, &[], &["0x00", "0x01"]),
            (
                "p18f4550",
                4096,
                &["INDF0", "INDF1", "INDF2"],
                &["0xFEF", "0xFE7", "0xFDF"],
            ),
            ("p18f4520", 4096, &[], &["0xFEF", "0xFE7", "0xFDF"]),
            ("p12f509", 256, &[], &["0x00"]),
        ];
        for &(name, ram_len, names, addrs) in cases {
            let dev = resolve_device(Some(name)).unwrap();
            for item in names.iter().chain(addrs) {
                let e = resolve_traces(dev, ram_len, &[item.to_string()]).unwrap_err();
                assert!(e.contains("INDF is not traceable"), "{name} {item}: {e}");
            }
        }
    }

    #[test]
    fn pseudo_registers_resolve_on_every_core() {
        let dev = resolve_device(None).unwrap();
        let traced = resolve_traces(dev, 512, &["0x02".into()]).unwrap();
        assert_eq!(traced[0].kind, TraceKind::Pcl);

        let dev = resolve_device(Some("p16f1937")).unwrap();
        for (item, kind) in [
            ("PCL", TraceKind::Pcl),
            ("WREG", TraceKind::Wreg),
            ("0x02", TraceKind::Pcl),
            ("0x09", TraceKind::Wreg),
        ] {
            let traced = resolve_traces(dev, 4096, &[item.into()]).unwrap();
            assert_eq!(traced[0].kind, kind, "p16f1937 {item}");
        }

        let dev = resolve_device(Some("p18f4550")).unwrap();
        for (item, kind) in [
            ("PCL", TraceKind::Pcl),
            ("WREG", TraceKind::Wreg),
            ("0xFF9", TraceKind::Pcl),
            ("0xFE8", TraceKind::Wreg),
        ] {
            let traced = resolve_traces(dev, 4096, &[item.into()]).unwrap();
            assert_eq!(traced[0].kind, kind, "p18f4550 {item}");
        }

        // Baseline has no SFR table: PCL still resolves by address, and
        // an unnamed address displays with two digits, not three.
        let dev = resolve_device(Some("p12f509")).unwrap();
        let traced = resolve_traces(dev, 256, &["0x02".into()]).unwrap();
        assert_eq!(traced[0].kind, TraceKind::Pcl);
        let traced = resolve_traces(dev, 256, &["0x20".into()]).unwrap();
        assert_eq!(traced[0].display, "0x20");
    }

    #[test]
    fn wreg_and_pcl_traces_snapshot_the_accessors() {
        let dev = resolve_device(Some("p16f1937")).unwrap();
        // movlw 0x05; sleep
        let mut sim = Sim::new(dev, vec![0x3005, 0x0063]);
        let traced = resolve_traces(dev, sim.ram().len(), &["WREG".into(), "PCL".into()]).unwrap();
        let (outcome, lines) = run_traced(&mut sim, 100, &traced);
        assert_eq!(outcome.stop, Stop::Halted, "lines: {lines:?}");
        assert!(
            lines.iter().any(|l| l.contains(" write WREG=0x05")),
            "lines: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains(" write PCL=")),
            "lines: {lines:?}"
        );
    }

    #[test]
    fn trace_and_dump_args_parse() {
        let a = parse_args(&argv(&["a.hex", "--trace-sfr", "PORTB,0x06", "--dump=ram"])).unwrap();
        assert_eq!(a.trace_sfr, ["PORTB", "0x06"]);
        assert_eq!(a.dump, Some(DumpKind::Ram));
        let e = parse_args(&argv(&["a.hex", "--dump", "everything"])).unwrap_err();
        assert!(e.contains(USAGE), "{e}");
    }
}
