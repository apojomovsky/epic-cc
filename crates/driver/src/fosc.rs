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
        Core::Pic14 => pic14_hz(&device.config, &fuse, xtal),
        Core::Pic18 => pic18_hz(&device.config, &fuse, xtal),
        Core::Pic14e => panic!("fosc: pic14e core not yet implemented for {}", device.name),
    }
}

pub fn resolve_fosc_hz_from_defaults(_device: &Device) -> u64 {
    // Oscillator-tree fields have no default (docs/31 D-9). Without an
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
            "epic-cc: xtal_hz=<Hz> is required in EPIC_CONFIG for PIC16F877A \
             (DS39582C §14.2: Fosc is the crystal or the declared RC frequency)"
        )
    })
}

fn pic18_hz(region: &ConfigRegion, spec: &str, xtal: Option<u64>) -> u64 {
    let osc = named(region, spec, "osc");
    let cpudiv = named(region, spec, "cpudiv");
    let pll = matches!(osc.as_str(), "hspll" | "xtpll" | "ecpll" | "ecpio");
    if matches!(osc.as_str(), "inths" | "intxt" | "intcko" | "intio") {
        // DS39632E §2.2.5: INTOSC is an 8 MHz clock that directly drives
        // the device clock in the internal-oscillator microcontroller
        // modes. CPUDIV applies only to XT/HS/EC and the PLL modes
        // (Register 25-1), not to INTOSC. Confirmed identical for both
        // devices this core currently ships (p18f4550, p18f2550): one
        // datasheet, one family (epic-cc#226). A future PIC18 outside
        // the 2455/2550/4455/4550 family (a K/Q/J-series part, whose
        // INTOSC is independently documented as configurable, not fixed
        // at 8 MHz) would need this re-verified, not assumed.
        return 8_000_000;
    }
    if pll {
        let plldiv = named(region, spec, "plldiv");
        let factor = plldiv_factor(&plldiv);
        let xtal = xtal.unwrap_or_else(|| {
            panic!(
                "epic-cc: xtal_hz=<Hz> is required in EPIC_CONFIG when osc={osc} \
                 (the PLL needs a known 4 MHz input, DS39632E §2.2.4)"
            )
        });
        if xtal / factor != 4_000_000 || xtal % factor != 0 {
            panic!(
                "epic-cc: xtal_hz={xtal} with plldiv={plldiv} does not produce the \
                 PLL's required 4 MHz input (DS39632E Register 25-1 / §2.2.4)"
            );
        }
        // Register 25-1, PLL modes: CPUDIV 00/01/10/11 = 96 MHz / 2,3,4,6.
        96_000_000 / pll_cpu_div(&cpudiv)
    } else {
        let xtal = xtal.unwrap_or_else(|| {
            panic!(
                "epic-cc: xtal_hz=<Hz> is required in EPIC_CONFIG when osc={osc} \
                 (DS39632E Register 25-1: system clock is the primary oscillator \
                 divided by CPUDIV)"
            )
        });
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
        other => panic!("epic-cc: unknown plldiv {other:?}"),
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

fn osc_cpu_div(name: &str) -> u64 {
    match name {
        "div1" => 1,
        "div2" => 2,
        "div3" => 3,
        "div4" => 4,
        other => panic!("epic-cc: unknown cpudiv {other:?} for a non-PLL oscillator mode"),
    }
}
