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

/// Test fixture: a D-2-like map, with or without the carving clips.
fn probe_device(banks: &'static [(u16, u16)], clips: bool) -> device::Device {
    if clips {
        device::Device {
            ram_banks: banks,
            isr_w_shadow: Some(0x20),
            isr_home_window: Some((0x70, 0x7F)),
            ..device::PIC16F877A
        }
    } else {
        device::Device {
            ram_banks: banks,
            common_ram: None,
            isr_home_window: None,
            isr_w_shadow: None,
            ..device::PIC16F877A
        }
    }
}

#[test]
fn probe_reason_none_when_placeable() {
    // The landed p16f74 shape: an 80-byte probe places in the second
    // window, so clips or none, the helper stays silent.
    let placeable: &[(u16, u16)] = &[(0x21, 0x6F), (0xA1, 0xFF)];
    assert!(probe_unplaceable_reason(&probe_device(placeable, false), 80).is_none());
    assert!(probe_unplaceable_reason(&probe_device(placeable, true), 80).is_none());
}

#[test]
fn alloc_empty_prog_does_not_panic() {
    for dev in devices_under_test() {
        let m = ir::parse("fn main(void) ()\n  block entry:\n    ret void\n");
        let _ = alloc::allocate(dev, &m, "depth 1\n");
    }
}

/// R5 (epic-cc#411): when reserved-span clips fragment the map below the
/// sanity probe, fail naming the clip instead of alloc's generic
/// no-arrangement panic. Mirrors alloc's align-at-most-two rule from the
/// first bank start, so placeable probes (every shipped device) return
/// None and alloc's own errors stand everywhere else.
fn probe_unplaceable_reason(dev: &device::Device, size: u16) -> Option<String> {
    let align = u32::from(size.min(2));
    let placeable = dev.ram_banks.iter().any(|&(lo, hi)| {
        let lo = u32::from(lo);
        let base = if lo % align == 0 {
            lo
        } else {
            lo + (align - (lo % align))
        };
        base + u32::from(size) - 1 <= u32::from(hi)
    });
    if placeable {
        return None;
    }
    let mut clips = Vec::new();
    if let Some((lo, hi)) = dev.common_ram {
        clips.push(format!("common_ram {lo:#06X}-{hi:#06X}"));
    }
    if let Some((lo, hi)) = dev.isr_home_window {
        clips.push(format!("isr_home_window {lo:#06X}-{hi:#06X}"));
    }
    if let Some(w) = dev.isr_w_shadow {
        clips.push(format!("isr_w_shadow {w:#06X}"));
    }
    if clips.is_empty() {
        return None;
    }
    Some(format!(
        "{}: {size}-byte probe fits no ram_banks window after the {} clip (R5 fragmentation floor, epic-cc#411)",
        dev.name,
        clips.join(", "),
    ))
}

#[test]
fn eighty_byte_global_lands_in_ram_banks() {
    for dev in devices_under_test() {
        let mut m = ir::parse("global big i8\nfn main(void) ()\n  block entry:\n    ret void\n");
        // The probe scales to the device: 80 bytes covers the first PIC14
        // bank on every device shipped before PIC16F84 (docs/39 D-1, whose
        // ram_banks is only 52 bytes). A global never spans banks (the
        // 509's 32 GPR bytes sit in two 16-byte banks), so size also caps
        // at the largest single window. alloc aligns every 2+-byte value to
        // an even address, so an odd-start window holds one byte fewer;
        // the trim belongs on the window cap before the 80-byte cap, or a
        // device whose widest window exceeds 80 probes 79 instead of 80.
        let capacity: u16 = dev.ram_banks.iter().map(|&(lo, hi)| hi - lo + 1).sum();
        let (widest, widest_lo) = dev
            .ram_banks
            .iter()
            .map(|&(lo, hi)| (hi - lo + 1, lo))
            .max()
            .unwrap_or((1, 0));
        let aligned_widest = widest - u16::from(widest_lo % 2 == 1);
        let size = capacity.min(80).min(aligned_widest).max(1);
        if let Some(reason) = probe_unplaceable_reason(dev, size) {
            panic!("{reason}");
        }
        m.globals[0].size = size;
        let layout = alloc::allocate(dev, &m, "depth 1\n");
        let addr = *layout.globals.get("big").expect("big global missing");
        assert!(
            dev.region_for(addr).is_some(),
            "{}: {size}-byte global start {:#06x} not in ram_banks {:?}",
            dev.name,
            addr,
            dev.ram_banks
        );
        let end = addr + size - 1;
        assert!(
            dev.region_for(end).is_some(),
            "{}: {size}-byte global end {:#06x} not in ram_banks {:?}",
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

#[test]
fn probe_reason_names_clip_when_unplaceable() {
    let small: &[(u16, u16)] = &[(0x21, 0x2F), (0xA1, 0xAF)];
    let r = probe_unplaceable_reason(&probe_device(small, true), 80).expect("x");
    assert!(r.contains("isr_home_window"), "{r}");
    assert!(r.contains("isr_w_shadow"), "{r}");
}
