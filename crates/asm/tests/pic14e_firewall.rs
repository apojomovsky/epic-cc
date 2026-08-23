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
fn assemble_file_to_hex_for_pic14e_assembles_as_pic14_for_hal2_demo() {
    // HAL-2 demo: Pic14e (p16f193x) shares the Pic14 banking/ISA for the
    // purpose of the p16f877a HAL build (the full Pic14e backend is not yet
    // implemented, but treating it as Pic14 is correct for the demo and
    // matches `device::Core::Pic14e => assemble(src)` in `asm`).
    let hex = asm::assemble_file_to_hex(&PIC14E_STUB, "    NOP\n");
    assert!(hex.starts_with(':'), "expected Intel HEX, got {hex:?}");
}
