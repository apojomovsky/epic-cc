//! The size report the driver prints to stderr after every hex build.
//!
//! "RAM used" is the bytes of RAM the program's allocation occupies: the
//! per-bank high-water marks from the overlay layout plus the fixed
//! scratch/retval/ISR-save region isel reserves. Overlay allocation makes
//! this less obvious than on a stack machine, since a byte can be live in
//! several frames, so the report states the definition on the line.

use alloc::AllocLayout;
use device::Device;

/// The address map file: `global <name> 0xNN`, `const <name>` (flash, no
/// RAM address), and `local <key> 0xNN` where `<key>` is the driver's
/// `{func}::{name}` HashMap key, all sorted deterministically. The map is
/// the artifact a user reads when a program does not fit and they have to
/// decide what to cut.
pub fn map_text(device: &Device, layout: &AllocLayout) -> String {
    let mut out = String::new();
    out.push_str(&format!("; epic-cc map for {}\n", device.name));
    let mut globals: Vec<&String> = layout.globals.keys().collect();
    globals.sort();
    for name in globals {
        out.push_str(&format!("global {name} 0x{:02X}\n", layout.globals[name]));
    }
    let mut consts: Vec<&String> = layout.const_globals.iter().collect();
    consts.sort();
    for name in consts {
        out.push_str(&format!("const {name}\n"));
    }
    let mut locals: Vec<&String> = layout.locals.keys().collect();
    locals.sort();
    for key in locals {
        out.push_str(&format!("local {key} 0x{:02X}\n", layout.locals[key]));
    }
    out
}

/// The fixed bytes isel reserves outside the overlay: PIC14's common-RAM
/// scratch (1) + retval (4), plus the ISR save area (9) when the program
/// has an ISR. PIC18's access-bank retval/flag region (4), plus the ISR
/// save area (12) when the program has an ISR. These are isel's layout
/// constants (crates/isel/src/lib.rs, crates/isel-pic18/src/lib.rs).
pub fn fixed_bytes(device: &Device, has_isr: bool) -> u16 {
    match device.core {
        device::Core::Pic14 => {
            let base = 1 + 4; // scratch + retval
            if has_isr {
                base + 9
            } else {
                base
            }
        }
        device::Core::Pic18 => {
            let base = 4; // retval + flag bit
            if has_isr {
                base + 12
            } else {
                base
            }
        }
        device::Core::Pic14e => 0,
    }
}

/// The fixed region's total capacity: PIC14 common RAM, PIC18's access
/// bank (the fixed_retval reservation is a policy slice of it).
pub fn fixed_total(device: &Device) -> u16 {
    match device.core {
        device::Core::Pic14 => {
            let (lo, hi) = device
                .common_ram
                .expect("PIC14 devices have a common-RAM region");
            hi - lo + 1
        }
        device::Core::Pic18 => {
            let (lo, hi) = device
                .access_bank
                .expect("PIC18 devices have an access bank");
            hi - lo + 1
        }
        device::Core::Pic14e => 0,
    }
}

/// Render the size report. `flash_used` is the program's assembled word
/// count (before config-word insertion); `layout` carries the RAM facts.
pub fn render_size(device: &Device, layout: &AllocLayout, flash_used: usize) -> String {
    let mut out = String::new();
    out.push_str(&format!("epic-cc: program size for {}:\n", device.name));
    out.push_str(&format!(
        "  flash: {flash_used}/{} words ({:.1}%)\n",
        device.flash_words,
        flash_used as f64 * 100.0 / device.flash_words as f64
    ));
    let ram_total: u16 = device
        .ram_banks
        .iter()
        .map(|&(s, e)| e - s + 1)
        .sum::<u16>()
        + fixed_total(device);
    let ram_used: u16 =
        layout.bank_used.iter().sum::<u16>() + fixed_bytes(device, layout.isr_bytes > 0);
    out.push_str(&format!(
        "  RAM: {ram_used}/{ram_total} bytes ({:.1}%) (overlay: a byte can be live in several frames; used = the bytes of RAM the program's allocation occupies)\n",
        ram_used as f64 * 100.0 / ram_total as f64
    ));
    for (i, &used) in layout.bank_used.iter().enumerate() {
        let (start, end) = device.ram_banks[i];
        let total = end - start + 1;
        out.push_str(&format!("    bank {i}: {used}/{total} bytes\n"));
    }
    let fixed = fixed_bytes(device, layout.isr_bytes > 0);
    let fixed_total = fixed_total(device);
    let fixed_name = match device.core {
        device::Core::Pic14 => "common",
        device::Core::Pic18 => "fixed",
        device::Core::Pic14e => "fixed",
    };
    out.push_str(&format!(
        "    {fixed_name}: {fixed}/{fixed_total} bytes (fixed scratch/retval/ISR save)\n"
    ));
    if layout.isr_bytes > 0 {
        out.push_str(&format!(
            "    ISR region: {} bytes (disjoint, after the main context, included in the bank totals)\n",
            layout.isr_bytes
        ));
    }
    out
}
