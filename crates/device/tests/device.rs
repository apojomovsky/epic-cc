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
    assert_eq!(PIC16F877A.region_for(0x1A0), Some((0x190, 0x1FF)));
}

#[test]
fn region_for_returns_none_past_the_last_bank() {
    assert_eq!(PIC16F877A.region_for(0x400), None);
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

#[test]
fn pic18f4550_reserves_retval_and_isr_save_regions() {
    // 0x0000-0x0003 = the fixed retval region (P2); 0x0004-0x000F = the
    // fixed ISR save area (P5, see the P5 plan Task 3); GPR starts at
    // 0x0010 so nothing overlaps the reservations.
    assert_eq!(PIC18F4550.common_ram, Some((0x0000, 0x000F)));
    assert_eq!(PIC18F4550.gpr_start(), 0x0010);
    assert_eq!(PIC18F4550.region_for(0x0010), Some((0x0010, 0x07FF)));
    assert_eq!(PIC18F4550.region_for(0x07FF), Some((0x0010, 0x07FF)));
    assert_eq!(
        PIC18F4550.region_for(0x0800),
        None,
        "past the implemented GPR range"
    );
}

#[test]
fn by_name_resolves_both_devices() {
    assert_eq!(device::by_name("p16f877a").unwrap().name, "p16f877a");
    assert_eq!(device::by_name("p18f4550").unwrap().name, "p18f4550");
    assert_eq!(device::by_name("p16f887").unwrap().name, "p16f887");
    assert!(
        device::by_name("P16F877A").is_none(),
        "by_name is case-sensitive; driver lowercases"
    );
}

#[test]
fn all_contains_both_seed_devices() {
    assert_eq!(device::ALL.len(), 3);
    assert!(device::ALL.iter().any(|d| d.name == "p16f877a"));
    assert!(device::ALL.iter().any(|d| d.name == "p18f4550"));
    assert!(device::ALL.iter().any(|d| d.name == "p16f887"));
}

#[test]
fn by_name_case_insensitive_helper() {
    assert_eq!(
        device::by_name_case_insensitive("P16F877A").unwrap().name,
        "p16f877a"
    );
    assert_eq!(
        device::by_name_case_insensitive("p18F4550").unwrap().name,
        "p18f4550"
    );
}

#[test]
fn every_device_exposes_an_sfr_table() {
    // The table is the contract epic-hal generates its per family SFR headers
    // from. The compiler never reads an SFR name, so it stays empty here; what
    // matters is that the field exists and survives codegen.
    for d in device::ALL {
        assert!(d.sfrs.is_empty(), "{} ships a non-empty sfrs table", d.name);
    }
}

#[test]
fn fuse_masks_are_contiguous_at_their_shift() {
    // build.rs rejects a mask that is not `width` contiguous bits at `shift`.
    // Assert the shipped data satisfies the invariant the generator enforces,
    // so a regression in either surfaces here rather than at resolve time.
    for d in device::ALL {
        for f in d.config.fields {
            let width = f.mask.count_ones();
            let expected = (((1u16 << width) - 1) << f.shift) as u16;
            assert_eq!(
                f.mask as u16, expected,
                "{}: field {} mask {:#04X} is not {} bit(s) at shift {}",
                d.name, f.name, f.mask, width, f.shift
            );
        }
    }
}

#[test]
fn flash_words_is_a_power_of_two() {
    for d in device::ALL {
        assert!(
            d.flash_words.is_power_of_two(),
            "{}: flash_words {} is not a power of two",
            d.name,
            d.flash_words
        );
    }
}
