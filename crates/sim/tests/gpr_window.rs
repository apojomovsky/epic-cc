//! Banked GPR is decided by the device's memory map, not by a fixed operand
//! window. Bank 2 and 3 SFRs are shorter than bank 0's on the PIC14 parts, so
//! their GPR starts below 0x20 and a fixed `0x20-0x6F` window cannot reach it.

use device::{ConfigRegion, Core, Device};
use pic14_sim::Pic14;

// MOVLW k = 0x3000 | k; MOVWF f = 0x0080 | f; BSF f,b = 0x1400 | (b<<7) | f.
// STATUS = 0x03, RP0 = bit 5, RP1 = bit 6.
const BSF_RP0: u16 = 0x1400 | (5 << 7) | 0x03;
const BSF_RP1: u16 = 0x1400 | (6 << 7) | 0x03;

/// A PIC14 part whose banks 2 and 3 start at their true SFR boundary. Shaped
/// like the 877A otherwise; the shipped tables start those two banks 16 bytes
/// late, which is corrected separately.
const WIDE_BANKS: Device = Device {
    name: "p16f877a-wide",
    core: Core::Pic14,
    flash_words: 0x2000,
    ram_banks: &[(0x20, 0x6F), (0xA0, 0xEF), (0x110, 0x16F), (0x190, 0x1EF)],
    common_ram: Some((0x70, 0x7F)),
    access_bank: None,
    fixed_retval: None,
    stack_depth: 8,
    interrupt_vectors: &[0x0004],
    config: ConfigRegion {
        base_byte_addr: 0x400E,
        num_bytes: 2,
        erased_baseline: &[0xFF, 0x3F],
        fields: &[],
    },
    sfrs: &[],
};

fn run(device: &'static Device, prog: Vec<u16>) -> Pic14 {
    let mut p = Pic14::with_device(device, prog);
    p.run(1000);
    p
}

#[test]
fn an_operand_below_0x20_is_gpr_where_the_bank_says_so() {
    // RP1=1 (bank 2), MOVWF 0x10 -> physical 0x110, which this device calls GPR.
    let p = run(&WIDE_BANKS, vec![BSF_RP1, 0x30A5, 0x0080 | 0x10]);
    assert_eq!(p.ram()[0x110], 0xA5, "store must land at physical 0x110");
    assert_eq!(p.ram()[0x10], 0x00, "the bank 0 SFR must be untouched");
}

#[test]
fn the_same_operand_in_bank_0_is_still_an_sfr() {
    let p = run(&WIDE_BANKS, vec![0x305A, 0x0080 | 0x10]);
    assert_eq!(p.ram()[0x10], 0x5A, "bank 0 f=0x10 is the SFR at 0x10");
    assert_eq!(p.ram()[0x110], 0x00, "no banked GPR cell is written");
}

#[test]
fn bank_3_gpr_starts_at_its_own_boundary() {
    let p = run(&WIDE_BANKS, vec![BSF_RP1, BSF_RP0, 0x303C, 0x0080 | 0x10]);
    assert_eq!(p.ram()[0x190], 0x3C);
    assert_eq!(p.ram()[0x10], 0x00);
}

#[test]
fn common_ram_ignores_the_bank() {
    // 0x70-0x7F is mirrored in every bank, so it resolves to its bank 0 offset
    // whatever RP1:RP0 says.
    let p = run(&WIDE_BANKS, vec![BSF_RP1, BSF_RP0, 0x3011, 0x0080 | 0x75]);
    assert_eq!(p.ram()[0x75], 0x11);
    assert_eq!(p.ram()[0x1F5], 0x00, "no banked cell shadows common RAM");
}

#[test]
fn the_shipped_device_resolves_its_banks_unchanged() {
    // The mechanism must not move anything on the canonical device.
    let p = run(&device::PIC16F877A, vec![BSF_RP0, 0x3077, 0x0080 | 0x20]);
    assert_eq!(p.ram()[0xA0], 0x77, "bank 1 f=0x20 is physical 0xA0");
    assert_eq!(p.ram()[0x20], 0x00);
}
