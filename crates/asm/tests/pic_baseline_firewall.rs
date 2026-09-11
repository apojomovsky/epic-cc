//! The `pic-baseline` core's encoder landed in P1 (docs/37), so
//! `assemble_file_to_hex` now emits real 12-bit words for it. This test
//! pins the round-trip through the shared `assemble_words`/`to_hex` path
//! on the p12f509 stub, the same gate the `pic14e` core ran behind until
//! its P2.

use device::{ConfigRegion, Core, Device};

const PIC_BASELINE_STUB: Device = Device {
    name: "p12f509-stub",
    core: Core::PicBaseline,
    flash_words: 0x400,
    ram_banks: &[(0x10, 0x1F), (0x30, 0x3F)],
    common_ram: Some((0x07, 0x0F)),
    access_bank: None,
    fixed_retval: None,
    isr_w_shadow: None,
    isr_home_window: None,
    fsr_bank_bits: 1,
    stack_depth: 2,
    interrupt_vectors: &[],
    config: ConfigRegion {
        base_byte_addr: 0x1FFE,
        num_bytes: 2,
        erased_baseline: &[0xFF, 0x0F],
        fields: &[],
    },
    sfrs: &[],
};

#[test]
fn assemble_file_to_hex_emits_pic_baseline_words() {
    // MOVLW 0x10 -> 0x0C10; MOVWF FSR -> 0x0024; BSF FSR,5 -> 0x05A4.
    // to_hex emits little-endian byte pairs, so word 0x0C10 reads "100C".
    let hex = asm::assemble_file_to_hex(
        &PIC_BASELINE_STUB,
        "    MOVLW 0x10\n    MOVWF 0x04\n    BSF 0x04, 5\n",
    );
    assert!(
        hex.contains("100C"),
        "MOVLW 0x10 encodes to 0x0C10, got:\n{hex}"
    );
    assert!(
        hex.contains("2400"),
        "MOVWF 0x04 encodes to 0x0024, got:\n{hex}"
    );
    assert!(
        hex.contains("A405"),
        "BSF 0x04,5 encodes to 0x05A4, got:\n{hex}"
    );
}
