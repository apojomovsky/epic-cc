//! Derive `EPIC_FOSC_HZ` from an `EPIC_CONFIG` spec.
//!
//! Arithmetic is from DS39582C §14.2 (PIC16F877A) and DS39632E §2.2 /
//! Register 25-1 (PIC18F4550). `xtal_hz` is not a silicon bit; it is
//! stripped before `resolve_config` sees the spec.

use device::{ConfigRegion, Core, Device, FuseField};

/// Split `xtal_hz=<n>` out of an EPIC_CONFIG spec. The remainder is a
/// fuse-only string `resolve_config` can consume.
pub fn split_xtal_hz(spec: &str) -> (String, Option<u64>) {
    let mut xtal = None;
    let mut rest = Vec::new();
    for pair in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (key, val) = pair.split_once('=').unwrap_or((pair, ""));
        let (key, val) = (key.trim(), val.trim());
        if key.eq_ignore_ascii_case("xtal_hz") {
            xtal = Some(val.parse::<u64>().unwrap_or_else(|_| {
                panic!("epic-cc: xtal_hz value {val:?} is not an integer frequency in Hz")
            }));
        } else {
            rest.push(format!("{key}={val}"));
        }
    }
    (rest.join(", "), xtal)
}

pub fn fuse_spec(spec: &str) -> String {
    split_xtal_hz(spec).0
}

/// System clock in Hz from a full EPIC_CONFIG spec (may include `xtal_hz`).
pub fn resolve_fosc_hz(device: &Device, spec: &str) -> u64 {
    let (fuse, xtal) = split_xtal_hz(spec);
    // Validate the fuse half (required oscillator fields, locked, etc.).
    let _ = device::resolve_config(&device.config, &fuse);
    match device.core {
        Core::Pic14 | Core::Pic14e => pic14_hz(&device.config, &fuse, xtal),
        Core::Pic18 => pic18_hz(&device.config, &fuse, xtal),
        Core::PicBaseline => baseline_hz(&device.config, &fuse, xtal),
    }
}

pub fn resolve_fosc_hz_from_defaults(_device: &Device) -> u64 {
    // Oscillator-tree fields have no default (docs/31 §9). Without an
    // EPIC_CONFIG the driver cannot know the board's crystal, so the
    // preprocessor macro is the inert 0 from epic-cc.h. Existing fixtures
    // have no EPIC_CONFIG and must keep compiling.
    0
}

fn named(region: &ConfigRegion, spec: &str, field: &str) -> String {
    for pair in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim().eq_ignore_ascii_case(field) {
                return v.trim().to_ascii_lowercase();
            }
        }
    }
    let f = field_of(region, field);
    f.default
        .unwrap_or_else(|| {
            panic!("epic-cc: field '{field}' has no default and was not set by EPIC_CONFIG")
        })
        .to_ascii_lowercase()
}

fn field_of<'a>(region: &'a ConfigRegion, name: &str) -> &'a FuseField {
    region
        .fields
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("epic-cc: no fuse field '{name}' on this device"))
}

fn pic14_hz(region: &ConfigRegion, spec: &str, xtal: Option<u64>) -> u64 {
    // DS39582C §14.2.1: LP/XT/HS/RC. Crystal modes have no PLL; Fosc is
    // the crystal. RC frequency is a function of R, C, Vdd and temperature
    // (§14.2.3) and cannot be derived, so the user must still declare it
    // as xtal_hz.
    let _osc = named(region, spec, "osc");
    xtal.unwrap_or_else(|| {
        panic!(
            "epic-cc: xtal_hz=<Hz> is required in EPIC_CONFIG \
             (DS39582C §14.2: Fosc is the crystal or the declared RC frequency)"
        )
    })
}

fn baseline_hz(region: &ConfigRegion, spec: &str, xtal: Option<u64>) -> u64 {
    // DS41236E §2.0: the 509's internal oscillator is a 4 MHz precision
    // internal oscillator (INTRC). The `osc` fuse field selects LP/XT/
    // INTOSC/EXTRC; the internal modes run at 4 MHz, the crystal/RC modes
    // need the declared xtal_hz (Fosc is the crystal or the RC frequency,
    // which cannot be derived).
    let osc = named(region, spec, "osc");
    if matches!(osc.as_str(), "intosc") {
        4_000_000
    } else {
        xtal.unwrap_or_else(|| {
            panic!(
                "epic-cc: xtal_hz=<Hz> is required in EPIC_CONFIG when osc={osc} \
                 (DS41236E §2.0: Fosc is the crystal or the declared RC frequency)"
            )
        })
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

fn pic18_hz(region: &ConfigRegion, spec: &str, xtal: Option<u64>) -> u64 {
    let osc = named(region, spec, "osc");
    // Confirmed on PIC18F252/258/452/2525/4520/4620: the non-USB PIC18
    // family (DS39025/DS39631) has no CPUDIV/PLLDIV/USBDIV fuses at all;
    // Register 25-1's configurable divider chain is specific to the 96 MHz
    // USB PLL family (2455/2550/4455/4550) already shipped. Reading cpudiv
    // is deferred into the branch that actually needs it so a device
    // without the fuse never has to have one just to resolve a non-PLL
    // clock.
    if matches!(
        osc.as_str(),
        "inths" | "intxt" | "intcko" | "intio" | "intio67" | "intio7" | "intosc"
    ) {
        // DS39632E §2.2.5 / DS39631E §2.2.4 / DS30009964C §1.1.3: INTOSC is
        // an 8 MHz clock that directly drives the device clock in the
        // internal-oscillator microcontroller modes on every PIC18 family
        // that has one. CPUDIV applies only to XT/HS/EC and the PLL modes
        // (Register 25-1), not to INTOSC (epic-cc#226).
        return 8_000_000;
    }
    if matches!(osc.as_str(), "intoscpll" | "intoscpllo") {
        // DS30009964C §3.2.4: the PLL can also run from INTOSC in these
        // modes, PLLEN-gated at runtime (OSCTUNE<6>), not by a Configuration
        // fuse this function can read. Resolving a compile-time Fosc for
        // that combination needs the exact INTOSC-to-PLL-input path
        // confirmed first; refuse rather than guess a number.
        panic!(
            "epic-cc: osc={osc} is not yet supported for EPIC_FOSC_HZ (the PLL's \
             enable state here is a runtime OSCTUNE bit, not a Configuration fuse)"
        );
    }
    let pll = matches!(osc.as_str(), "hspll" | "xtpll" | "ecpll" | "ecpio");
    if pll {
        let xtal = xtal.unwrap_or_else(|| {
            panic!(
                "epic-cc: xtal_hz=<Hz> is required in EPIC_CONFIG when osc={osc} \
                 (the PLL needs a known input frequency)"
            )
        });
        if !has_field(region, "plldiv") {
            // DS39631E Register 24-1 / DS39025 §2.2.2: this family's HSPLL
            // is a fixed 4x multiplier ahead of the CPU, no PLLDIV/CPUDIV
            // prescaler or postscaler at all.
            return xtal * 4;
        }
        let plldiv = named(region, spec, "plldiv");
        let factor = plldiv_factor(&plldiv);
        if xtal / factor != 4_000_000 || xtal % factor != 0 {
            panic!(
                "epic-cc: xtal_hz={xtal} with plldiv={plldiv} does not produce the \
                 PLL's required 4 MHz input (DS39632E Register 25-1 / §2.2.4)"
            );
        }
        let cpudiv = named(region, spec, "cpudiv");
        if is_j_series_pll(region) {
            // DS30009964C Register 28-2 / Table: CPDIV<1:0> divides the
            // fixed 48 MHz USB clock (11/10/01/00 = /1,/2,/3,/6), not the
            // raw 96 MHz PLL reference.
            return 48_000_000 / cpudiv_suffix_divisor(&cpudiv);
        }
        // Register 25-1, PLL modes: CPUDIV 00/01/10/11 = 96 MHz / 2,3,4,6.
        96_000_000 / pll_cpu_div(&cpudiv)
    } else {
        let xtal = xtal.unwrap_or_else(|| {
            panic!(
                "epic-cc: xtal_hz=<Hz> is required in EPIC_CONFIG when osc={osc} \
                 (system clock is the primary oscillator, possibly divided by CPUDIV)"
            )
        });
        if !has_field(region, "cpudiv") {
            // DS39631E / DS39025: no CPUDIV fuse on this family; the
            // primary oscillator drives the system clock directly.
            return xtal;
        }
        let cpudiv = named(region, spec, "cpudiv");
        if is_j_series_pll(region) {
            // Register 28-2's CPDIV<1:0> table is not qualified by
            // oscillator mode; with no PLL engaged it divides the raw
            // primary oscillator by the same /1,/2,/3,/6 set instead of
            // 48 MHz.
            return xtal / cpudiv_suffix_divisor(&cpudiv);
        }
        // Register 25-1, XT/HS/EC/ECIO: CPUDIV 00/01/10/11 = OSC / 1,2,3,4.
        xtal / osc_cpu_div(&cpudiv)
    }
}

fn plldiv_factor(name: &str) -> u64 {
    match name {
        "noprescale" => 1,
        "div2" => 2,
        "div3" => 3,
        "div4" => 4,
        "div5" => 5,
        "div6" => 6,
        "div10" => 10,
        "div12" => 12,
        // DS30009964C Register 28-1: PIC18F27J53's PLLDIV is spelled as a
        // bare divisor number ("1".."12"), not a div-prefixed name.
        other => other
            .parse()
            .unwrap_or_else(|_| panic!("epic-cc: unknown plldiv {other:?}")),
    }
}

fn pll_cpu_div(name: &str) -> u64 {
    match name {
        "div1" => 2,
        "div2" => 3,
        "div3" => 4,
        "div4" => 6,
        other => panic!("epic-cc: unknown cpudiv {other:?} for a PLL oscillator mode"),
    }
}

/// DS30009964C Register 28-2: PIC18F27J53's CPUDIV values are spelled
/// "oscK" (divide by 1) or "oscK_pllN" (divide by N); the leading oscK is
/// an unrelated internal index, confirmed against PIC18F2550's identically
/// suffixed raw values (osc1_pll2, osc2_pll3, osc3_pll4, osc4_pll6, aliased
/// in gen-device.py to divide by 2/3/4/6 respectively, matching the N here
/// too) even though the two families' actual divisor sets differ.
fn cpudiv_suffix_divisor(name: &str) -> u64 {
    match name.rsplit_once("_pll") {
        Some((_, n)) => n
            .parse()
            .unwrap_or_else(|_| panic!("epic-cc: unknown cpudiv {name:?}")),
        None if name.starts_with("osc") => 1,
        None => panic!("epic-cc: unknown cpudiv {name:?}"),
    }
}

fn osc_cpu_div(name: &str) -> u64 {
    match name {
        "div1" => 1,
        "div2" => 2,
        "div3" => 3,
        "div4" => 4,
        other => panic!("epic-cc: unknown cpudiv {other:?} for a non-PLL oscillator mode"),
    }
}
