use device::{PIC16F877A, PIC18F4550};
use driver::fosc::resolve_fosc_hz;

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
#[should_panic(expected = "xtal_hz")]
fn pic18_pll_xtal_not_matching_plldiv_panics() {
    // 8 MHz crystal with PLLDIV=div5 does not produce the PLL's required
    // 4 MHz input (DS39632E §2.2.4 / Register 25-1).
    resolve_fosc_hz(
        &PIC18F4550,
        "osc=hspll, plldiv=div5, cpudiv=div1, usbdiv=on, xtal_hz=8000000",
    );
}
