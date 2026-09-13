//! epic-cc#420: pins p16f74's bucket-2 memory model (docs/39): bank0's
//! 0x70-0x7F (the ISR home window) and bank1's 0xF0-0xFF are distinct
//! cells, so bank1 keeps its full A1-FF extent and no mirror clip
//! applies to this part.

use device::Core;
use device::{ConfigRegion, Device};
use pic14_sim::Pic14;

// MOVLW k = 0x3000 | k; MOVWF f = 0x0080 | f; MOVF f,W = 0x0800 | f.
// BSF/BCF STATUS,RP0 = 0x1400/0x1000 | (5<<7) | 0x03.
const BSF_RP0: u16 = 0x1400 | (5 << 7) | 0x03;
const BCF_RP0: u16 = 0x1000 | (5 << 7) | 0x03;

/// The shipped p16f74 map: two real regions, the ISR home window carved
/// out of region A, nothing carved from region B.
const P16F74: Device = Device {
    name: "p16f74-probe",
    core: Core::Pic14,
    flash_words: 4096,
    ram_banks: &[(0x21, 0x6F), (0xA1, 0xFF)],
    common_ram: None,
    access_bank: None,
    fixed_retval: None,
    isr_w_shadow: Some(0x20),
    isr_home_window: Some((0x70, 0x7F)),
    stack_depth: 8,
    fsr_bank_bits: 0,
    interrupt_vectors: &[0x0004],
    config: ConfigRegion {
        base_byte_addr: 0x2007,
        num_bytes: 1,
        erased_baseline: &[0xFF],
        fields: &[],
    },
    sfrs: &[],
};

fn run(prog: Vec<u16>) -> Pic14 {
    let len = prog.len() as u16;
    let mut p = Pic14::with_device(&P16F74, prog);
    p.run_until(len, 1000);
    p
}
#[test]
fn writing_bank1_f0_does_not_disturb_bank0_70() {
    // Bank1: W=0xAB -> 0xF0. Bank0: read 0x70. Distinct regions, so the
    // read sees the initial zero, never the value bank1 wrote.
    let p = run(vec![BSF_RP0, 0x30AB, 0x0080 | 0xF0, BCF_RP0, 0x0800 | 0x70]);
    assert_eq!(p.ram()[0xF0], 0xAB, "bank1 store must land at 0xF0");
    assert_eq!(p.w(), 0x00, "0x70 must not alias 0xF0");
}

#[test]
fn writing_bank0_70_does_not_disturb_bank1_f0() {
    // Seed bank1's 0xF0, then write bank0's 0x70 (the ISR home
    // window) in a fresh program carrying the RAM state over.
    // Region B keeps its value: distinct cells.
    let mut p = Pic14::with_device(&P16F74, vec![BSF_RP0, 0x30AB, 0x0080 | 0xF0]);
    p.run_until(3, 1000);
    assert_eq!(p.ram()[0xF0], 0xAB, "bank1 store must land at 0xF0");
    let mut q = Pic14::with_device(&P16F74, vec![BCF_RP0, 0x30CD, 0x0080 | 0x70]);
    q.ram_mut().copy_from_slice(p.ram());
    q.run_until(3, 1000);
    assert_eq!(q.ram()[0x70], 0xCD, "bank0 store must land at 0x70");
    assert_eq!(
        q.ram()[0xF0],
        0xAB,
        "0xF0 must keep its value: real region-B cell"
    );
}
