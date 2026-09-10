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
    assert_eq!(PIC16F877A.region_for(0x1A0), Some((0x190, 0x1EF)));
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
fn bank_of_banks_a_non_mirrored_bank0_sfr() {
    // Only the six core registers (INDF/PCL/STATUS/FSR/PCLATH/INTCON) are
    // mirrored into every bank. A non-mirrored bank-0 SFR (PORTA 0x05,
    // TMR0 0x01, ...) exists solely in bank 0, so it needs RP1:RP0 = 0.
    assert_eq!(PIC16F877A.bank_of(0x00), None); // INDF, mirrored
    assert_eq!(PIC16F877A.bank_of(0x02), None); // PCL, mirrored
    assert_eq!(PIC16F877A.bank_of(0x04), None); // FSR, mirrored
    assert_eq!(PIC16F877A.bank_of(0x0A), None); // PCLATH, mirrored
    assert_eq!(PIC16F877A.bank_of(0x0B), None); // INTCON, mirrored
    assert_eq!(PIC16F877A.bank_of(0x01), Some(0)); // TMR0, not mirrored
    assert_eq!(PIC16F877A.bank_of(0x05), Some(0)); // PORTA, not mirrored
    assert_eq!(PIC16F877A.bank_of(0x06), Some(0)); // PORTB, not mirrored
    assert_eq!(PIC16F877A.bank_of(0x10), Some(0)); // T1CON, not mirrored
}
#[test]
fn bank_of_banks_a_high_bank_sfr() {
    // An SFR above the first GPR bank is paged by RP1:RP0 like a GPR is, so
    // it needs a BANKSEL: the 887's ANSEL/ANSELH are the motivating case.
    assert_eq!(PIC16F877A.bank_of(0x180), Some(3)); // INDF, bank 3
    assert_eq!(PIC16F877A.bank_of(0x188), Some(3)); // ANSEL on the 887
    assert_eq!(PIC16F877A.bank_of(0x85), Some(1)); // TRISA
    assert_eq!(PIC16F877A.bank_of(0x10F), Some(2));
}

#[test]
#[should_panic(expected = "0x0F0 aliases common RAM 0x70")]
fn bank_of_panics_on_a_common_ram_alias() {
    // 0xF0 is 0x70 seen from bank 1: common RAM has one canonical spelling
    // and no stage may emit an alias of it.
    PIC16F877A.bank_of(0xF0);
}

#[test]
fn pic14e_mirrors_the_full_core_register_block() {
    // DS41364E Table 3-3 (and the memory maps in Table 3-5/3-8, which the
    // 1937's 32 banks repeat): every bank's first 12 bytes are INDF0,
    // INDF1, PCL, STATUS, FSR0L, FSR0H, FSR1L, FSR1H, BSR, WREG, PCLATH,
    // INTCON, addressable without a bank switch. Every other bank-0 SFR is
    // not mirrored (PORTA 0x0C exists solely in bank 0).
    let dev = device::by_name("p16f1937").unwrap();
    assert_eq!(dev.core, Core::Pic14e);
    for addr in 0x00..=0x0B {
        assert_eq!(
            dev.bank_of(addr),
            None,
            "0x{addr:02X} must be bank-independent on pic14e"
        );
    }
    assert_eq!(dev.bank_of(0x0C), Some(0)); // PORTA, not mirrored
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
    // `fixed_retval` is the policy reservation inside the hardware
    // `access_bank` (issue #109): common_ram is PIC14 only.
    assert_eq!(PIC18F4550.common_ram, None);
    assert_eq!(PIC18F4550.access_bank, Some((0x0000, 0x005F)));
    assert_eq!(PIC18F4550.fixed_retval, Some((0x0000, 0x000F)));
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
fn all_contains_every_seed_device() {
    assert_eq!(device::ALL.len(), 28);
    assert!(device::ALL.iter().any(|d| d.name == "p16f877a"));
    assert!(device::ALL.iter().any(|d| d.name == "p18f4550"));
    assert!(device::ALL.iter().any(|d| d.name == "p16f887"));
    assert!(device::ALL.iter().any(|d| d.name == "p18f2550"));
    assert!(device::ALL.iter().any(|d| d.name == "p16f628a"));
    for stem in [
        "p16f1933",
        "p16f1934",
        "p16f1936",
        "p16f1937",
        "p16f1938",
        "p16f1939",
        "p12f509",
        "p16f747",
        "p16f87",
        "p16f877",
        "p16f88",
        "p18f252",
        "p18f2525",
        "p18f258",
        "p18f452",
        "p18f4520",
        "p18f4620",
        "p18f26k22",
        "p18lf46k22",
        "p18f27j53",
        "p18f66j94",
        "p18f87j11",
        "p12f683",
    ] {
        assert!(
            device::ALL.iter().any(|d| d.name == stem),
            "{stem} missing from ALL"
        );
    }
}

#[test]
fn pic12f509_carries_the_baseline_profile() {
    let dev = device::by_name("p12f509").unwrap();
    assert_eq!(dev.core, Core::PicBaseline);
    assert_eq!(dev.flash_words, 1024);
    assert_eq!(dev.stack_depth, 2);
    assert_eq!(dev.fsr_bank_bits, 1);
    assert_eq!(dev.interrupt_vectors, &[][..]);
    assert_eq!(dev.ram_banks, &[(0x10, 0x1F), (0x30, 0x3F)][..]);
    assert_eq!(dev.common_ram, Some((0x07, 0x0F)));
}

#[test]
fn pic12f509_bank_of_knows_the_shared_window_and_both_banks() {
    // DS41236E Figure 4-4: the SFR block 0x00-0x06 and the shared GPR
    // 0x07-0x0F are mirrored in both banks; bank 0 owns 0x10-0x1F and
    // bank 1 owns 0x30-0x3F (bank select = FSR<5>, fsr_bank_bits = 1).
    let dev = device::by_name("p12f509").unwrap();
    for addr in 0x00..=0x0F {
        assert_eq!(
            dev.bank_of(addr),
            None,
            "0x{addr:02X} must be bank-independent on the 509"
        );
    }
    assert_eq!(dev.bank_of(0x10), Some(0));
    assert_eq!(dev.bank_of(0x1F), Some(0));
    assert_eq!(dev.bank_of(0x30), Some(1));
    assert_eq!(dev.bank_of(0x3F), Some(1));
}

#[test]
#[should_panic(expected = "not a banked GPR address")]
fn pic12f509_bank_of_refuses_the_protected_alias_window() {
    // 0x20-0x2F is the bank-1 view of the mirrored 0x00-0x0F block
    // (gputils: protected SHAREBANK); it has no canonical GPR meaning.
    device::by_name("p12f509").unwrap().bank_of(0x25);
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
fn fuse_masks_agree_with_shift_and_hold_their_values() {
    // build.rs rejects a mask whose lowest set bit is not `shift` and a
    // value that does not fit inside the mask once shifted. Masks may be
    // scattered (PIC16F628A FOSC is 0x13). Assert the shipped data satisfies
    // the invariant the generator enforces, so a regression in either
    // surfaces here rather than at resolve time.
    for d in device::ALL {
        for f in d.config.fields {
            assert_eq!(
                f.mask.trailing_zeros() as u8,
                f.shift,
                "{}: field {} mask {:#04X} lowest set bit is not shift {}",
                d.name,
                f.name,
                f.mask,
                f.shift
            );
            for v in f.values {
                assert_eq!(
                    ((v.bits as u16) << f.shift) & !(f.mask as u16),
                    0,
                    "{}: field {} value {} bits {} outside mask {:#04X}",
                    d.name,
                    f.name,
                    v.name,
                    v.bits,
                    f.mask
                );
            }
        }
    }
}

#[test]
fn pic14_flash_words_is_a_multiple_of_the_page_size() {
    // isel/isel-pic14e page CALL/GOTO in 0x800-word blocks; pic18 (21-bit
    // PC, no paging) and pic-baseline (isel-pic-baseline pages
    // differently) have no such requirement. Confirmed on PIC18F2525's
    // real, datasheet-stated 24576-word flash: not a power of two either.
    for d in device::ALL {
        if matches!(d.core, device::Core::Pic14 | device::Core::Pic14e) {
            assert!(
                d.flash_words % 0x800 == 0,
                "{}: flash_words {} is not a multiple of 0x800",
                d.name,
                d.flash_words
            );
        }
    }
}

#[test]
fn resolve_accepts_every_spelling_the_ecosystem_uses() {
    // epic-cc's own stem, epic-hal's manifest variant, XC8's -mcpu, and MPLAB's
    // part define, all naming one part.
    for spelling in ["p16f877a", "P16F877A", "16F877A", "16f877a", "PIC16F877A"] {
        assert_eq!(
            device::resolve(spelling).map(|d| d.name),
            Some("p16f877a"),
            "{spelling} should resolve"
        );
    }
    assert_eq!(
        device::resolve("  16F887  ").map(|d| d.name),
        Some("p16f887")
    );
    assert_eq!(device::resolve("18f4550").map(|d| d.name), Some("p18f4550"));
}

#[test]
fn resolve_rejects_a_part_we_do_not_ship() {
    assert!(device::resolve("p99f9999").is_none());
    assert!(device::resolve("16F1935").is_none());
    assert!(device::resolve("").is_none());
}

#[test]
fn by_name_stays_exact() {
    // resolve() is the forgiving front door; by_name is the exact lookup the
    // generated table provides, and callers depend on that distinction.
    assert!(device::by_name("P16F877A").is_none());
    assert!(device::by_name("16f877a").is_none());
    assert!(device::by_name("p16f877a").is_some());
}

#[test]
fn pic14_banks_cover_the_full_368_bytes_of_gpr() {
    // gputils p16f877a.inc: bank 2's last SFR is EEADRH at 0x10F and bank 3's
    // is EECON2 at 0x18D (0x18E-0x18F are __BADRAM), so GPR starts at 0x110
    // and 0x190. The 16 bytes above each bank mirror common RAM. DS39582C
    // section 2.2 quotes 368 bytes; the 887 (DS41291D) is identical.
    let banked: u32 = PIC16F877A
        .ram_banks
        .iter()
        .map(|&(lo, hi)| (hi - lo + 1) as u32)
        .sum();
    let (clo, chi) = PIC16F877A.common_ram.unwrap();
    assert_eq!(banked + (chi - clo + 1) as u32, 368);

    // the bytes the old tables dropped
    assert_eq!(PIC16F877A.bank_of(0x110), Some(2));
    assert_eq!(PIC16F877A.bank_of(0x11F), Some(2));
    assert_eq!(PIC16F877A.bank_of(0x190), Some(3));
    assert_eq!(PIC16F877A.bank_of(0x19F), Some(3));
}

#[test]
fn p16f887_has_the_same_gpr_geometry_as_the_877a() {
    let d = device::by_name("p16f887").unwrap();
    assert_eq!(d.ram_banks, PIC16F877A.ram_banks);
    assert_eq!(d.common_ram, PIC16F877A.common_ram);
}
