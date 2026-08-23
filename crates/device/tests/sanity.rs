//! Per-device lightweight sanity: schema via build.rs plus alloc and asm checks.
//!
//! This is the cheap per-device gate from the CI stratification design
//! (spec 2026-08-22 §8). For each `devices/*.toml` it verifies:
//! - `alloc` empty-program does not panic,
//! - one 80-byte global lands inside `ram_banks`,
//! - `asm` flash-bound accepts a tiny program.
//!
//! Run for one device: `SANITY_DEVICE=p16f877a cargo test -p device --test sanity`
//! Run for all devices: `cargo test -p device --test sanity`
//! The `scripts/sanity.sh` helper and the `devices-changed` CI job use the
//! single-device form.

use device::{by_name, ALL};

fn devices_under_test() -> Vec<&'static device::Device> {
    if let Ok(filter) = std::env::var("SANITY_DEVICE") {
        let d = by_name(&filter).unwrap_or_else(|| {
            panic!(
                "sanity: unknown SANITY_DEVICE={filter:?}, known: {}",
                ALL.iter().map(|d| d.name).collect::<Vec<_>>().join(", ")
            )
        });
        vec![d]
    } else {
        ALL.to_vec()
    }
}

#[test]
fn alloc_empty_prog_does_not_panic() {
    for dev in devices_under_test() {
        let m = ir::parse("fn main(void) ()\n  block entry:\n    ret void\n");
        let _ = alloc::allocate(dev, &m, "depth 1\n");
    }
}

#[test]
fn eighty_byte_global_lands_in_ram_banks() {
    for dev in devices_under_test() {
        let mut m = ir::parse("global big i8\nfn main(void) ()\n  block entry:\n    ret void\n");
        // Force an 80-byte global (covers the first PIC14 bank exactly: 0x20-0x6F).
        m.globals[0].size = 80;
        let layout = alloc::allocate(dev, &m, "depth 1\n");
        let addr = *layout.globals.get("big").expect("big global missing");
        assert!(
            dev.region_for(addr).is_some(),
            "{}: 80-byte global start {:#06x} not in ram_banks {:?}",
            dev.name,
            addr,
            dev.ram_banks
        );
        let end = addr + 79;
        assert!(
            dev.region_for(end).is_some(),
            "{}: 80-byte global end {:#06x} not in ram_banks {:?}",
            dev.name,
            end,
            dev.ram_banks
        );
    }
}

#[test]
fn asm_flash_bound_accepts_tiny_program() {
    for dev in devices_under_test() {
        // Minimal program: one NOP at org 0. NOP (0x0000) is valid on both
        // PIC14 and PIC18 and avoids label resolution (GOTO with a literal
        // trips the PIC18 label table).
        let src = "    org 0x0000\n    nop\n";
        let hex = asm::assemble_file_to_hex(dev, src);
        assert!(
            !hex.is_empty(),
            "{}: tiny program produced empty hex",
            dev.name
        );
        assert!(
            hex.contains(':'),
            "{}: tiny program hex missing Intel HEX records",
            dev.name
        );
    }
}
