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
    stack_depth: 0,
    fsr_bank_bits: 0,
    interrupt_vectors: &[],
    config: device::ConfigRegion {
        base_byte_addr: 0,
        num_bytes: 1,
        erased_baseline: &[0xFF],
        fields: &[FuseField {
            name: "osc",
            byte_offset: 0,
            mask: 0x07,
            shift: 0,
            values: &[
                FuseValue {
                    name: "hs",
                    bits: 2,
                },
                FuseValue {
                    name: "hspll",
                    bits: 6,
                },
            ],
            default: None,
            locked: None,
        }],
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
#[should_panic(expected = "xtal_hz")]
fn pic14_xt_without_xtal_hz_panics() {
    resolve_fosc_hz(&PIC16F877A, "osc=xt");
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
