use device::{Core, Device, FuseField, FuseValue, PIC16F877A, PIC18F4550};
use driver::fosc::resolve_fosc_hz;

/// Confirmed on PIC18F252/258/452/2525/4520/4620: the non-USB PIC18 family
/// has no CPUDIV/PLLDIV/USBDIV fuses at all, unlike PIC18F4550's Register
/// 25-1. No shipped device exercises that shape yet, so this hand-built
/// `Device` stands in for one: only `core` and `config` matter to
/// `resolve_fosc_hz`, so every other field is an inert placeholder.
const NO_CPUDIV_PIC18: Device = Device {
    name: "test-no-cpudiv",
    core: Core::Pic18,
    flash_words: 0,
    ram_banks: &[(0, 0)],
    common_ram: None,
    access_bank: Some((0, 0)),
    fixed_retval: Some((0, 0)),
    isr_w_shadow: None,
    isr_home_window: None,
    stack_depth: 0,
    fsr_bank_bits: 0,
    interrupt_vectors: &[],
    config: device::ConfigRegion {
        base_byte_addr: 0,
        num_bytes: 1,
        erased_baseline: &[0xFF],
        fields: &[FuseField {
            name: "osc",
            aliases: &[],
            byte_offset: 0,
            mask: 0x07,
            shift: 0,
            values: &[
                FuseValue {
                    name: "hs",
                    aliases: &[],
                    bits: 2,
                },
                FuseValue {
                    name: "hspll",
                    aliases: &[],
                    bits: 6,
                },
            ],
            default: None,
            locked: None,
        }],
    },
    sfrs: &[],
};

/// Confirmed on PIC18F27J53/66J94/87J11 (DS30009964C): has PLLDIV and
/// CPUDIV like PIC18F4550's Register 25-1, but no USBDIV fuse at all (a
/// fixed 48 MHz USB tap instead of a selectable one), CPUDIV's raw values
/// spelled as bare divisors ("osc1", "osc2_pll2", ...) rather than
/// "div1".."div4", and PLLDIV spelled as a bare number rather than
/// "noprescale"/"div2".. No shipped device exercises this shape yet.
const J_SERIES_PIC18: Device = Device {
    name: "test-j-series",
    core: Core::Pic18,
    flash_words: 0,
    ram_banks: &[(0, 0)],
    common_ram: None,
    access_bank: Some((0, 0)),
    fixed_retval: Some((0, 0)),
    isr_w_shadow: None,
    isr_home_window: None,
    stack_depth: 0,
    fsr_bank_bits: 0,
    interrupt_vectors: &[],
    config: device::ConfigRegion {
        base_byte_addr: 0,
        num_bytes: 1,
        erased_baseline: &[0xFF],
        fields: &[
            FuseField {
                name: "osc",
                aliases: &[],
                byte_offset: 0,
                mask: 0x07,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "hs",
                        aliases: &[],
                        bits: 4,
                    },
                    FuseValue {
                        name: "hspll",
                        aliases: &[],
                        bits: 5,
                    },
                    FuseValue {
                        name: "intosc",
                        aliases: &[],
                        bits: 0,
                    },
                    FuseValue {
                        name: "intosco",
                        aliases: &[],
                        bits: 1,
                    },
                    FuseValue {
                        name: "intoscpll",
                        aliases: &[],
                        bits: 2,
                    },
                ],
                default: None,
                locked: None,
            },
            FuseField {
                name: "plldiv",
                aliases: &[],
                byte_offset: 0,
                mask: 0x38,
                shift: 3,
                values: &[
                    FuseValue {
                        name: "1",
                        aliases: &[],
                        bits: 7,
                    },
                    FuseValue {
                        name: "4",
                        aliases: &[],
                        bits: 4,
                    },
                ],
                default: Some("1"),
                locked: None,
            },
            FuseField {
                name: "cpudiv",
                aliases: &[],
                byte_offset: 0,
                mask: 0xC0,
                shift: 6,
                values: &[
                    FuseValue {
                        name: "osc1",
                        aliases: &[],
                        bits: 3,
                    },
                    FuseValue {
                        name: "osc2_pll2",
                        aliases: &[],
                        bits: 2,
                    },
                    FuseValue {
                        name: "osc3_pll3",
                        aliases: &[],
                        bits: 1,
                    },
                    FuseValue {
                        name: "osc4_pll6",
                        aliases: &[],
                        bits: 0,
                    },
                ],
                default: Some("osc1"),
                locked: None,
            },
        ],
    },
    sfrs: &[],
};

#[test]
fn pic14_xt_is_the_crystal_frequency() {
    // DS39582C §14.2.1: XT/HS/LP are crystal modes with no PLL or
    // postscaler. EPIC_FOSC_HZ is the oscillator frequency, matching
    // the _XTAL_FREQ / F_CPU convention.
    let hz = resolve_fosc_hz(&PIC16F877A, "osc=xt, xtal_hz=4000000");
    assert_eq!(hz, 4_000_000);
}

#[test]
fn pic18_hspll_20mhz_div5_cpudiv1_is_48mhz() {
    // DS39632E Register 25-1: HSPLL enables the PLL, which produces a
    // fixed 96 MHz from a 4 MHz input (PLLDIV=div5 on a 20 MHz crystal).
    // CPUDIV=div1 in PLL modes is 96 MHz / 2 = 48 MHz system clock.
    let hz = resolve_fosc_hz(
        &PIC18F4550,
        "osc=hspll, plldiv=div5, cpudiv=div1, usbdiv=on, xtal_hz=20000000",
    );
    assert_eq!(hz, 48_000_000);
}

#[test]
fn pic18_hs_no_pll_cpudiv2_divides_the_crystal() {
    // DS39632E Register 25-1: for HS (no PLL), CPUDIV=div2 is
    // primary oscillator / 2.
    let hz = resolve_fosc_hz(
        &PIC18F4550,
        "osc=hs, plldiv=div5, cpudiv=div2, usbdiv=off, xtal_hz=20000000",
    );
    assert_eq!(hz, 10_000_000);
}

#[test]
fn pic14_xt_without_xtal_hz_is_unknown_until_needed() {
    // docs/46 D-4: no source at all is fine until something needs the
    // clock; the crystal-less config fixes nothing, so hz stays 0.
    let hz = resolve_fosc_hz(&PIC16F877A, "osc=xt");
    assert_eq!(hz, 0);
}

#[test]
fn pic18_hs_without_cpudiv_fuse_is_the_bare_crystal() {
    // DS39631E / DS39025: no CPUDIV fuse on this family, so the primary
    // oscillator drives the system clock directly.
    let hz = resolve_fosc_hz(&NO_CPUDIV_PIC18, "osc=hs, xtal_hz=10000000");
    assert_eq!(hz, 10_000_000);
}

#[test]
fn pic18_hspll_without_plldiv_fuse_is_a_fixed_4x() {
    // DS39631E Register 24-1 / DS39025 §2.2.2: this family's HSPLL is a
    // fixed 4x multiplier ahead of the CPU, no PLLDIV/CPUDIV at all.
    let hz = resolve_fosc_hz(&NO_CPUDIV_PIC18, "osc=hspll, xtal_hz=4000000");
    assert_eq!(hz, 16_000_000);
}

#[test]
#[should_panic(expected = "xtal_hz")]
fn pic18_pll_xtal_not_matching_plldiv_panics() {
    // 8 MHz crystal with PLLDIV=div5 does not produce the PLL's required
    // 4 MHz input (DS39632E §2.2.4 / Register 25-1).
    resolve_fosc_hz(
        &PIC18F4550,
        "osc=hspll, plldiv=div5, cpudiv=div1, usbdiv=on, xtal_hz=8000000",
    );
}

#[test]
fn j_series_intosc_is_8mhz() {
    let hz = resolve_fosc_hz(&J_SERIES_PIC18, "osc=intosc");
    assert_eq!(hz, 8_000_000);
}
#[test]
fn j_series_intosco_is_8mhz() {
    let hz = resolve_fosc_hz(&J_SERIES_PIC18, "osc=intosco");
    assert_eq!(hz, 8_000_000);
}

#[test]
fn j_series_hspll_divides_the_fixed_48mhz_usb_clock_not_96mhz() {
    // DS30009964C Register 28-2: no USBDIV fuse means a fixed 48 MHz USB
    // tap; CPUDIV divides that 48 MHz, not the 96 MHz PLL reference.
    let hz = resolve_fosc_hz(
        &J_SERIES_PIC18,
        "osc=hspll, plldiv=4, cpudiv=osc2_pll2, xtal_hz=16000000",
    );
    assert_eq!(hz, 24_000_000);
}

#[test]
fn j_series_hs_no_pll_divides_the_raw_crystal() {
    let hz = resolve_fosc_hz(
        &J_SERIES_PIC18,
        "osc=hs, cpudiv=osc3_pll3, xtal_hz=12000000",
    );
    assert_eq!(hz, 4_000_000);
}

#[test]
#[should_panic(expected = "not yet supported")]
fn j_series_intoscpll_refuses_rather_than_guesses() {
    resolve_fosc_hz(&J_SERIES_PIC18, "osc=intoscpll");
}

#[test]
fn pragma_without_a_derivable_clock_is_unknown() {
    // epic-cc#706 left the erased fallback in resolution; docs/46 D-4
    // turns the old missing-crystal error into an unknown clock: an 877A
    // pragma without FOSC reads the erased nibble (rc) and still needs a
    // crystal, while the strict spelling keeps reporting EPIC_CONFIG.
    let clock = driver::fosc::try_resolve_clock(
        &PIC16F877A,
        Some(("", device::ConfigSpelling::Pragma)),
        None,
        None,
    )
    .unwrap();
    assert_eq!(clock.hz, 0);
    let err = driver::fosc::try_resolve_clock(
        &PIC16F877A,
        Some(("", device::ConfigSpelling::EpicConfig)),
        None,
        None,
    )
    .unwrap_err();
    assert!(err.contains("EPIC_CONFIG"), "message: {err}");
}

#[test]
fn pragma_falls_through_to_xtal_freq() {
    // The 4550's erased osc nibble (0xF) names no value, so the config
    // fixes nothing and the code's _XTAL_FREQ wins without an error.
    let clock = driver::fosc::try_resolve_clock(
        &PIC18F4550,
        Some(("wdt=off", device::ConfigSpelling::Pragma)),
        Some(8_000_000),
        None,
    )
    .unwrap();
    assert_eq!(clock.hz, 8_000_000);
    assert_eq!(clock.from, Some(driver::fosc::ClockFrom::XtalFreq));
    assert_eq!(clock.mode, None);
}

#[test]
fn disagreeing_sources_fail_naming_both_values() {
    let err = driver::fosc::try_resolve_clock(
        &PIC16F877A,
        Some((
            "osc=xt, xtal_hz=4000000",
            device::ConfigSpelling::EpicConfig,
        )),
        Some(8_000_000),
        None,
    )
    .unwrap_err();
    assert!(err.contains("4000000"), "message: {err}");
    assert!(err.contains("8000000"), "message: {err}");
    assert!(err.contains("the config"), "message: {err}");
    assert!(err.contains("_XTAL_FREQ"), "message: {err}");
}

#[test]
fn agreeing_sources_resolve_once() {
    let clock = driver::fosc::try_resolve_clock(
        &PIC16F877A,
        Some((
            "osc=xt, xtal_hz=4000000",
            device::ConfigSpelling::EpicConfig,
        )),
        Some(4_000_000),
        Some(4_000_000),
    )
    .unwrap();
    assert_eq!(clock.hz, 4_000_000);
    assert_eq!(clock.from, Some(driver::fosc::ClockFrom::Config));
    assert_eq!(clock.mode.as_deref(), Some("xt"));
}

#[test]
fn f_cpu_is_the_last_resort_source() {
    let clock = driver::fosc::try_resolve_clock(&PIC16F877A, None, None, Some(16_000_000)).unwrap();
    assert_eq!(clock.hz, 16_000_000);
    assert_eq!(clock.from, Some(driver::fosc::ClockFrom::BoardCpu));
}

#[test]
fn intosc_on_pic14_parts_runs_at_4mhz() {
    // epic-cc#691: the 628A and 12F675 select the factory-calibrated 4 MHz
    // internal oscillator, which used to demand an xtal_hz and panic.
    for (part, value) in [("p16f628a", "intosc_noclkout"), ("p12f675", "intoscio")] {
        let dev = device::by_name(part).unwrap();
        let clock = driver::fosc::try_resolve_clock(
            dev,
            Some((
                &format!("osc={value}, pwrt=on"),
                device::ConfigSpelling::EpicConfig,
            )),
            None,
            None,
        )
        .unwrap();
        assert_eq!(clock.hz, 4_000_000, "{part}");
    }
}

#[test]
fn tunable_intosc_falls_through_instead_of_guessing() {
    // The 887 tunes its internal rate via OSCCON/IRCF, so its fuses fix
    // no rate: the declared crystal wins, and silence without one.
    let dev = device::by_name("p16f887").unwrap();
    let clock = driver::fosc::try_resolve_clock(
        dev,
        Some((
            "osc=intosc_noclkout, xtal_hz=8000000",
            device::ConfigSpelling::EpicConfig,
        )),
        None,
        None,
    )
    .unwrap();
    assert_eq!(clock.hz, 8_000_000);
    let clock = driver::fosc::try_resolve_clock(
        dev,
        Some(("osc=intosc_noclkout", device::ConfigSpelling::EpicConfig)),
        None,
        None,
    )
    .unwrap();
    assert_eq!(clock.hz, 0);
}

#[test]
fn contradictory_internal_xtal_fails_naming_both() {
    let dev = device::by_name("p16f628a").unwrap();
    let err = driver::fosc::try_resolve_clock(
        dev,
        Some((
            "osc=intosc_noclkout, pwrt=on, xtal_hz=8000000",
            device::ConfigSpelling::EpicConfig,
        )),
        None,
        None,
    )
    .unwrap_err();
    assert!(err.contains("4000000"), "message: {err}");
    assert!(err.contains("8000000"), "message: {err}");
}

#[test]
fn software_selected_hfintosc_falls_through_to_the_declared_rate() {
    // Bare `intosc` (the Enhanced family's HFINTOSC) names no fixed rate,
    // so the config fixes nothing: the declared crystal wins when present,
    // otherwise the clock stays unknown instead of guessing.
    let dev = device::by_name("p16f1937").unwrap();
    let clock = driver::fosc::try_resolve_clock(
        dev,
        Some((
            "osc=intosc, xtal_hz=16000000",
            device::ConfigSpelling::EpicConfig,
        )),
        None,
        None,
    )
    .unwrap();
    assert_eq!(clock.hz, 16_000_000);
    let clock = driver::fosc::try_resolve_clock(
        dev,
        Some(("osc=intosc", device::ConfigSpelling::EpicConfig)),
        None,
        None,
    )
    .unwrap();
    assert_eq!(clock.hz, 0);
}

#[test]
fn header_line_names_part_rate_and_mode() {
    let clock = driver::fosc::Clock {
        hz: 20_000_000,
        from: Some(driver::fosc::ClockFrom::Config),
        mode: Some("hs".to_string()),
    };
    assert_eq!(
        driver::fosc::header_line(&PIC16F877A, &clock),
        "epic-cc: PIC16F877A @ 20 MHz (HS)"
    );
    let unknown = driver::fosc::Clock {
        hz: 0,
        from: None,
        mode: None,
    };
    assert_eq!(
        driver::fosc::header_line(&PIC16F877A, &unknown),
        "epic-cc: PIC16F877A @ unknown"
    );
}
