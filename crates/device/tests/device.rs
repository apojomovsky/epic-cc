use device::{Core, PIC16F877A, PIC18F4550};

#[test]
fn gpr_start_is_the_first_banks_start() {
    assert_eq!(PIC16F877A.gpr_start(), 0x20);
}

#[test]
fn region_for_returns_the_containing_bank() {
    assert_eq!(PIC16F877A.region_for(0x20), Some((0x20, 0x6F)));
    assert_eq!(PIC16F877A.region_for(0x6F), Some((0x20, 0x6F)));
    assert_eq!(PIC16F877A.region_for(0xB0), Some((0xA0, 0xEF)));
    assert_eq!(PIC16F877A.region_for(0x1A0), Some((0x1A0, 0x1EF)));
}

#[test]
fn region_for_returns_none_past_the_last_bank() {
    assert_eq!(PIC16F877A.region_for(0x1F0), None);
}

#[test]
fn bank_of_identifies_each_gpr_bank() {
    assert_eq!(PIC16F877A.bank_of(0x20), Some(0));
    assert_eq!(PIC16F877A.bank_of(0xA5), Some(1));
    assert_eq!(PIC16F877A.bank_of(0x150), Some(2));
    assert_eq!(PIC16F877A.bank_of(0x1E0), Some(3));
}

#[test]
fn bank_of_is_none_for_sfr_and_common_ram() {
    assert_eq!(PIC16F877A.bank_of(0x03), None); // SFR (STATUS)
    assert_eq!(PIC16F877A.bank_of(0x75), None); // common RAM
}

#[test]
#[should_panic(expected = "0x180")]
fn bank_of_panics_on_an_unimplemented_gap() {
    PIC16F877A.bank_of(0x180); // the 0x170-0x19F unimplemented gap
}

#[test]
fn pic18f4550_profile_has_the_right_core_and_flash_size() {
    assert_eq!(PIC18F4550.core, Core::Pic18);
    assert_eq!(PIC18F4550.name, "p18f4550");
    // 0x007FFF is the last flash byte address (gputils' PIC18F4550-feat.html);
    // 32768 bytes / 2 bytes-per-word = 16384 = 0x4000 words.
    assert_eq!(PIC18F4550.flash_words, 0x4000);
}
