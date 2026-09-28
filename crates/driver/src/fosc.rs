//! The build clock, one value from whichever source the code gives
//! (docs/46 D-4): a clock derivable from the config (an internal
//! oscillator, or `EPIC_CONFIG` carrying `xtal_hz`), the code's
//! `_XTAL_FREQ`, or the board's `--f-cpu`. The first source present wins;
//! every other source present must agree with it, or the build fails
//! naming both values. No source at all is fine until something needs the
//! clock: `hz` is then 0, the inert `EPIC_FOSC_HZ` from `epic-cc.h`.
//!
//! Arithmetic is from DS39582C §14.2 (PIC16F877A) and DS39632E §2.2 /
//! Register 25-1 (PIC18F4550). `xtal_hz` is not a silicon bit; it is
//! stripped before `resolve_config` sees the spec.

use device::{ConfigRegion, Core, Device, FuseField};

use crate::diag;

/// Where a clock candidate came from, for agreement errors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClockFrom {
    Config,
    XtalFreq,
    BoardCpu,
}

impl ClockFrom {
    /// The source name for messages: `the config`, `_XTAL_FREQ`, `--f-cpu`.
    pub fn name(&self) -> &'static str {
        match self {
            ClockFrom::Config => "the config",
            ClockFrom::XtalFreq => "_XTAL_FREQ",
            ClockFrom::BoardCpu => "--f-cpu",
        }
    }
}

/// The resolved build clock. `hz` 0 with no source means unknown; `mode`
/// is the canonical oscillator value (`xt`, `intio`) when the config
/// fixed it, for the header line.
#[derive(Debug)]
pub struct Clock {
    pub hz: u64,
    pub from: Option<ClockFrom>,
    pub mode: Option<String>,
}

/// Resolve the clock from the first source present: the config, the
/// code's `_XTAL_FREQ`, the board's `--f-cpu`. A present source that
/// disagrees with the winner fails the build naming both values.
pub fn try_resolve_clock(
    device: &Device,
    spec: Option<(&str, device::ConfigSpelling)>,
    xtal_hz: Option<u64>,
    f_cpu_hz: Option<u64>,
) -> Result<Clock, String> {
    let derived = match spec {
        Some((s, spelling)) => {
            let (fuse, xtal) = try_split_xtal_hz(s)?;
            device::try_resolve_config_in(&device.config, &fuse, spelling)?;
            derive_config_hz(device, &fuse, xtal, spelling)?
        }
        None => None,
    };
    let (config_hz, mode) = match derived {
        Some((hz, mode)) => (hz, Some(mode)),
        None => (None, None),
    };
    let mut winner: Option<(u64, ClockFrom)> = None;
    for (hz, from) in [
        (config_hz, ClockFrom::Config),
        (xtal_hz, ClockFrom::XtalFreq),
        (f_cpu_hz, ClockFrom::BoardCpu),
    ] {
        let Some(hz) = hz else { continue };
        match winner {
            None => winner = Some((hz, from)),
            Some((w, _)) if w == hz => {}
            Some((w, f)) => {
                return Err(format!(
                    "clock disagreement: {} gives {w} Hz but {} gives {hz} Hz",
                    f.name(),
                    from.name()
                ));
            }
        }
    }
    match winner {
        Some((hz, from)) => Ok(Clock {
            hz,
            from: Some(from),
            mode,
        }),
        None => Ok(Clock {
            hz: 0,
            from: None,
            mode: None,
        }),
    }
}

/// The build header line, e.g. `epic-cc: PIC16F877A @ 20 MHz (HS)`.
pub fn header_line(device: &Device, clock: &Clock) -> String {
    let part = device
        .name
        .strip_prefix('p')
        .unwrap_or(device.name)
        .to_uppercase();
    let hz = match clock.hz {
        0 => "unknown".to_string(),
        h if h % 1_000_000 == 0 => format!("{} MHz", h / 1_000_000),
        h if h % 1_000 == 0 => format!("{} kHz", h / 1_000),
        h => format!("{h} Hz"),
    };
    match &clock.mode {
        Some(mode) => format!("epic-cc: PIC{part} @ {hz} ({})", mode.to_uppercase()),
        None => format!("epic-cc: PIC{part} @ {hz}"),
    }
}

fn derive_config_hz(
    device: &Device,
    fuse: &str,
    xtal: Option<u64>,
    spelling: device::ConfigSpelling,
) -> Result<Option<(Option<u64>, String)>, String> {
    match device.core {
        Core::Pic14 | Core::Pic14e => pic14_hz(device, fuse, xtal, spelling),
        Core::Pic18 => pic18_hz(&device.config, fuse, xtal, spelling),
        Core::PicBaseline => baseline_hz(&device.config, fuse, xtal, spelling),
    }
}

/// Split `xtal_hz=<n>` out of an EPIC_CONFIG spec. The remainder is a
/// fuse-only string `resolve_config` can consume.
pub fn split_xtal_hz(spec: &str) -> (String, Option<u64>) {
    try_split_xtal_hz(spec).unwrap_or_else(|e| panic!("{} {e}", diag::USER_PREFIX))
}

fn try_split_xtal_hz(spec: &str) -> Result<(String, Option<u64>), String> {
    let mut xtal = None;
    let mut rest = Vec::new();
    for pair in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (key, val) = pair.split_once('=').unwrap_or((pair, ""));
        let (key, val) = (key.trim(), val.trim());
        if key.eq_ignore_ascii_case("xtal_hz") {
            let hz: u64 = val
                .parse()
                .map_err(|_| format!("xtal_hz value {val:?} is not an integer frequency in Hz"))?;
            xtal = Some(hz);
        } else {
            rest.push(format!("{key}={val}"));
        }
    }
    Ok((rest.join(", "), xtal))
}

pub fn fuse_spec(spec: &str) -> String {
    split_xtal_hz(spec).0
}

/// System clock in Hz from a full EPIC_CONFIG spec (may include `xtal_hz`),
/// with no other source: the config alone, strictly spelled.
pub fn resolve_fosc_hz(device: &Device, spec: &str) -> u64 {
    try_resolve_clock(
        device,
        Some((spec, device::ConfigSpelling::EpicConfig)),
        None,
        None,
    )
    .map(|c| c.hz)
    .unwrap_or_else(|e| panic!("{} {e}", diag::USER_PREFIX))
}

fn named(
    region: &ConfigRegion,
    spec: &str,
    field: &str,
    spelling: device::ConfigSpelling,
) -> Result<String, String> {
    // The spec may spell fields and values either way (`osc=intio` or
    // `FOSC = INTOSCIO_EC`): match the pair key through the field's
    // aliases and return the value's canonical name, so the derived clock
    // is the same from `EPIC_CONFIG` and `#pragma config`. An unknown
    // value passes through raw; `resolve_config` reports it with options.
    let target = field_of(region, field)?;
    for pair in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((k, v)) = pair.split_once('=') {
            let k = k.trim();
            if k.eq_ignore_ascii_case(target.name)
                || target.aliases.iter().any(|a| a.eq_ignore_ascii_case(k))
            {
                let raw = v.trim();
                if let Some(canon) = target.values.iter().find(|c| {
                    c.name.eq_ignore_ascii_case(raw)
                        || c.aliases.iter().any(|a| a.eq_ignore_ascii_case(raw))
                }) {
                    return Ok(canon.name.to_ascii_lowercase());
                }
                return Ok(raw.to_ascii_lowercase());
            }
        }
    }
    if let Some(default) = target.default {
        return Ok(default.to_ascii_lowercase());
    }
    Err(format!(
        "field '{field}' has no default and was not set by {}",
        match spelling {
            device::ConfigSpelling::EpicConfig => "EPIC_CONFIG",
            device::ConfigSpelling::Pragma => "#pragma config",
        }
    ))
}

/// Like `named`, but an omitted defaultless field under `#pragma config`
/// resolves through the erased baseline instead of erroring: `Some` when
/// the erased pattern names a value, `None` when it names nothing, and
/// then the config fixes no clock and the next D-4 source wins.
fn named_or_none(
    region: &ConfigRegion,
    spec: &str,
    field: &str,
    spelling: device::ConfigSpelling,
) -> Result<Option<String>, String> {
    let target = field_of(region, field)?;
    let set = spec
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|pair| {
            pair.split_once('=').is_some_and(|(k, _)| {
                let k = k.trim();
                k.eq_ignore_ascii_case(target.name)
                    || target.aliases.iter().any(|a| a.eq_ignore_ascii_case(k))
            })
        });
    if !set && target.default.is_none() && spelling == device::ConfigSpelling::Pragma {
        return Ok(erased_value_name(region, target));
    }
    named(region, spec, field, spelling).map(Some)
}

/// The value name the erased baseline encodes for `target`, if any. Some
/// erased patterns name no value (an all-ones nibble past the last mode),
/// and then there is nothing to derive a clock from.
fn erased_value_name(region: &ConfigRegion, target: &FuseField) -> Option<String> {
    let byte = region.erased_baseline.get(target.byte_offset as usize)?;
    let placed = (*byte as u16) & (target.mask as u16);
    target
        .values
        .iter()
        .find(|v| ((v.bits as u16) << target.shift) & (target.mask as u16) == placed)
        .map(|v| v.name.to_ascii_lowercase())
}

fn field_of<'a>(region: &'a ConfigRegion, name: &str) -> Result<&'a FuseField, String> {
    device::find_field(region, name).ok_or_else(|| format!("no fuse field '{name}' on this device"))
}

/// The `osc` values selecting the classic family's factory-calibrated 4 MHz
/// internal oscillator (DS40044 for the 628A, DS41175 for the 12F675).
/// Parts whose SFRs carry an OSCCON with IRCF bits (the 887) select the
/// internal rate in software, so those values name no fixed rate and fall
/// through like bare `intosc` (the Enhanced HFINTOSC and P10-class parts,
/// likewise software-selected).
const INTOSC_4MHZ: &[&str] = &["intoscio", "intoscclk", "intosc_noclkout", "intosc_clkout"];

/// Whether the part tunes its internal oscillator in software: an OSCCON
/// register with IRCF bits means the fuses fix no rate.
fn has_tunable_intosc(device: &Device) -> bool {
    device.sfrs.iter().any(|sfr| {
        sfr.name.eq_ignore_ascii_case("OSCCON")
            && sfr.fields.iter().any(|f| f.name.starts_with("IRCF"))
    })
}

fn pic14_hz(
    device: &Device,
    spec: &str,
    xtal: Option<u64>,
    spelling: device::ConfigSpelling,
) -> Result<Option<(Option<u64>, String)>, String> {
    // DS39582C §14.2.1: LP/XT/HS/RC. Crystal modes have no PLL; Fosc is
    // the crystal. RC frequency is a function of R, C, Vdd and temperature
    // (§14.2.3) and cannot be derived, so without xtal_hz the config fixes
    // no clock and the next source wins.
    let region = &device.config;
    let Some(osc) = named_or_none(region, spec, "osc", spelling)? else {
        return Ok(None);
    };
    if INTOSC_4MHZ.contains(&osc.as_str()) && !has_tunable_intosc(device) {
        if let Some(xtal) = xtal {
            if xtal != 4_000_000 {
                return Err(format!(
                    "clock disagreement: the config internal oscillator gives 4000000 Hz \
                     but xtal_hz gives {xtal} Hz"
                ));
            }
        }
        return Ok(Some((Some(4_000_000), osc)));
    }
    Ok(Some((xtal, osc)))
}

fn baseline_hz(
    region: &ConfigRegion,
    spec: &str,
    xtal: Option<u64>,
    spelling: device::ConfigSpelling,
) -> Result<Option<(Option<u64>, String)>, String> {
    // DS41236E §2.0: the 509's internal oscillator is a 4 MHz precision
    // internal oscillator (INTRC). The `osc` fuse field selects LP/XT/
    // INTOSC/EXTRC; the internal modes run at 4 MHz, the crystal/RC modes
    // need the declared xtal_hz (Fosc is the crystal or the RC frequency,
    // which cannot be derived).
    let Some(osc) = named_or_none(region, spec, "osc", spelling)? else {
        return Ok(None);
    };
    if osc == "intosc" {
        Ok(Some((Some(4_000_000), osc)))
    } else {
        Ok(Some((xtal, osc)))
    }
}

fn has_field(region: &ConfigRegion, name: &str) -> bool {
    region
        .fields
        .iter()
        .any(|f| f.name.eq_ignore_ascii_case(name))
}

/// DS30009964C (PIC18F27J53 family): has CPUDIV and PLLDIV like the USB PLL
/// family below, but no USBDIV fuse at all (a fixed 48 MHz USB tap instead
/// of a selectable one), and CPUDIV divides that fixed 48 MHz rather than
/// the raw 96 MHz PLL reference. Confirmed via `usbdiv`'s absence, the one
/// structural difference between the two PLL-capable PIC18 families.
fn is_j_series_pll(region: &ConfigRegion) -> bool {
    has_field(region, "plldiv") && has_field(region, "cpudiv") && !has_field(region, "usbdiv")
}

fn pic18_hz(
    region: &ConfigRegion,
    spec: &str,
    xtal: Option<u64>,
    spelling: device::ConfigSpelling,
) -> Result<Option<(Option<u64>, String)>, String> {
    let Some(osc) = named_or_none(region, spec, "osc", spelling)? else {
        return Ok(None);
    };
    // Confirmed on PIC18F252/258/452/2525/4520/4620: the non-USB PIC18
    // family (DS39025/DS39631) has no CPUDIV/PLLDIV/USBDIV fuses at all;
    // Register 25-1's configurable divider chain is specific to the 96 MHz
    // USB PLL family (2455/2550/4455/4550) already shipped. Reading cpudiv
    // is deferred into the branch that actually needs it so a device
    // without the fuse never has to have one just to resolve a non-PLL
    // clock.
    if matches!(
        osc.as_str(),
        "inths" | "intxt" | "intcko" | "intio" | "intio67" | "intio7" | "intosc" | "intosco"
    ) {
        // DS39632E §2.2.5 / DS39631E §2.2.4 / DS30009964C §1.1.3: INTOSC is
        // an 8 MHz clock that directly drives the device clock in the
        // internal-oscillator microcontroller modes on every PIC18 family
        // that has one. CPUDIV applies only to XT/HS/EC and the PLL modes
        // (Register 25-1), not to INTOSC (epic-cc#226).
        return Ok(Some((Some(8_000_000), osc)));
    }
    if matches!(osc.as_str(), "intoscpll" | "intoscpllo") {
        // DS30009964C §3.2.4: the PLL can also run from INTOSC in these
        // modes, PLLEN-gated at runtime (OSCTUNE<6>), not by a Configuration
        // fuse this function can read. Resolving a compile-time Fosc for
        // that combination needs the exact INTOSC-to-PLL-input path
        // confirmed first; refuse rather than guess a number.
        return Err(format!(
            "osc={osc} is not yet supported for EPIC_FOSC_HZ (the PLL's \
             enable state here is a runtime OSCTUNE bit, not a Configuration fuse)"
        ));
    }
    let pll = matches!(osc.as_str(), "hspll" | "xtpll" | "ecpll" | "ecpio");
    if pll {
        let Some(xtal) = xtal else {
            return Ok(Some((None, osc)));
        };
        if !has_field(region, "plldiv") {
            // DS39631E Register 24-1 / DS39025 §2.2.2: this family's HSPLL
            // is a fixed 4x multiplier ahead of the CPU, no PLLDIV/CPUDIV
            // prescaler or postscaler at all.
            return Ok(Some((Some(xtal * 4), osc)));
        }
        let Some(plldiv) = named_or_none(region, spec, "plldiv", spelling)? else {
            return Ok(Some((None, osc)));
        };
        let factor = plldiv_factor(&plldiv)?;
        if xtal / factor != 4_000_000 || xtal % factor != 0 {
            return Err(format!(
                "xtal_hz={xtal} with plldiv={plldiv} does not produce the \
                 PLL's required 4 MHz input (DS39632E Register 25-1 / §2.2.4)"
            ));
        }
        let Some(cpudiv) = named_or_none(region, spec, "cpudiv", spelling)? else {
            return Ok(Some((None, osc)));
        };
        if is_j_series_pll(region) {
            // DS30009964C Register 28-2 / Table: CPDIV<1:0> divides the
            // fixed 48 MHz USB clock (11/10/01/00 = /1,/2,/3,/6), not the
            // raw 96 MHz PLL reference.
            return Ok(Some((
                Some(48_000_000 / cpudiv_suffix_divisor(&cpudiv)?),
                osc,
            )));
        }
        // Register 25-1, PLL modes: CPUDIV 00/01/10/11 = 96 MHz / 2,3,4,6.
        Ok(Some((Some(96_000_000 / pll_cpu_div(&cpudiv)?), osc)))
    } else {
        let Some(xtal) = xtal else {
            return Ok(Some((None, osc)));
        };
        if !has_field(region, "cpudiv") {
            // DS39631E / DS39025: no CPUDIV fuse on this family; the
            // primary oscillator drives the system clock directly.
            return Ok(Some((Some(xtal), osc)));
        }
        let Some(cpudiv) = named_or_none(region, spec, "cpudiv", spelling)? else {
            return Ok(Some((None, osc)));
        };
        if is_j_series_pll(region) {
            // Register 28-2's CPDIV<1:0> table is not qualified by
            // oscillator mode; with no PLL engaged it divides the raw
            // primary oscillator by the same /1,/2,/3,/6 set instead of
            // 48 MHz.
            return Ok(Some((Some(xtal / cpudiv_suffix_divisor(&cpudiv)?), osc)));
        }
        // Register 25-1, XT/HS/EC/ECIO: CPUDIV 00/01/10/11 = OSC / 1,2,3,4.
        Ok(Some((Some(xtal / osc_cpu_div(&cpudiv)?), osc)))
    }
}

fn plldiv_factor(name: &str) -> Result<u64, String> {
    match name {
        "noprescale" => Ok(1),
        "div2" => Ok(2),
        "div3" => Ok(3),
        "div4" => Ok(4),
        "div5" => Ok(5),
        "div6" => Ok(6),
        "div10" => Ok(10),
        "div12" => Ok(12),
        // DS30009964C Register 28-1: PIC18F27J53's PLLDIV is spelled as a
        // bare divisor number ("1".."12"), not a div-prefixed name.
        other => other
            .parse()
            .map_err(|_| format!("unknown plldiv {other:?}")),
    }
}

fn pll_cpu_div(name: &str) -> Result<u64, String> {
    match name {
        "div1" => Ok(2),
        "div2" => Ok(3),
        "div3" => Ok(4),
        "div4" => Ok(6),
        other => Err(format!(
            "unknown cpudiv {other:?} for a PLL oscillator mode"
        )),
    }
}

/// DS30009964C Register 28-2: PIC18F27J53's CPUDIV values are spelled
/// "oscK" (divide by 1) or "oscK_pllN" (divide by N); the leading oscK is
/// an unrelated internal index, confirmed against PIC18F2550's identically
/// suffixed raw values (osc1_pll2, osc2_pll3, osc3_pll4, osc4_pll6, aliased
/// in gen-device.py to divide by 2/3/4/6 respectively, matching the N here
/// too) even though the two families' actual divisor sets differ.
fn cpudiv_suffix_divisor(name: &str) -> Result<u64, String> {
    match name.rsplit_once("_pll") {
        Some((_, n)) => n.parse().map_err(|_| format!("unknown cpudiv {name:?}")),
        None if name.starts_with("osc") => Ok(1),
        None => Err(format!("unknown cpudiv {name:?}")),
    }
}

fn osc_cpu_div(name: &str) -> Result<u64, String> {
    match name {
        "div1" => Ok(1),
        "div2" => Ok(2),
        "div3" => Ok(3),
        "div4" => Ok(4),
        other => Err(format!(
            "unknown cpudiv {other:?} for a non-PLL oscillator mode"
        )),
    }
}
