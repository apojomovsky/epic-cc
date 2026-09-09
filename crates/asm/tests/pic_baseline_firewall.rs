//! The `pic-baseline` core has no backend, so every stage dispatching on
//! `Device::core` must refuse it rather than emit something. Same gate the
//! `pic14e` core ran behind until its P2. A refusal once swapped for a
//! silent fallthrough would ship another core's machine code.

use device::{ConfigRegion, Core, Device};

const PIC_BASELINE_STUB: Device = Device {
    name: "p12f509-stub",
    core: Core::PicBaseline,
    flash_words: 0x400,
    ram_banks: &[(0x10, 0x1F), (0x30, 0x3F)],
    common_ram: Some((0x07, 0x0F)),
    access_bank: None,
    fixed_retval: None,
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
#[should_panic(expected = "pic-baseline")]
fn assemble_file_to_hex_refuses_pic_baseline() {
    asm::assemble_file_to_hex(&PIC_BASELINE_STUB, "    NOP\n");
}
