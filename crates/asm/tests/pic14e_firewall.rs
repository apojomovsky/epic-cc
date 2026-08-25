//! The `pic14e` core has no backend, so every stage dispatching on
//! `Device::core` must refuse it rather than emit something. This refusal was
//! once swapped for a silent fallthrough to make a demo build, which would
//! have shipped bank code for the wrong core.

use device::{ConfigRegion, Core, Device};

const PIC14E_STUB: Device = Device {
    name: "p16f1937-stub",
    core: Core::Pic14e,
    flash_words: 0x4000,
    ram_banks: &[(0x20, 0x6F)],
    common_ram: Some((0x70, 0x7F)),
    access_bank: None,
    fixed_retval: None,
    stack_depth: 16,
    interrupt_vectors: &[0x0004],
    config: ConfigRegion {
        base_byte_addr: 0x8007,
        num_bytes: 2,
        erased_baseline: &[0xFF, 0xFF],
        fields: &[],
    },
    sfrs: &[],
};

#[test]
#[should_panic(expected = "pic14e")]
fn assemble_file_to_hex_refuses_pic14e() {
    asm::assemble_file_to_hex(&PIC14E_STUB, "    NOP\n");
}
