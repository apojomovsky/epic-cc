//! The size report the driver prints to stderr after every hex build.
//!
//! "RAM used" is the bytes of RAM the program's allocation occupies: the
//! per-bank high-water marks from the overlay layout plus the fixed
//! scratch/retval/ISR-save region isel reserves. Overlay allocation makes
//! this less obvious than on a stack machine, since a byte can be live in
//! several frames, so the report states the definition on the line.

use alloc::AllocLayout;
use device::Device;
use ir::SrcLoc;

/// The address-to-source-line table: one `file:line:col <addr>` record per
/// word of the final program, sorted by address. Compiler-generated words
/// (no source instruction) are omitted. Built by walking the final asm
/// text with the same pass-1 semantics `asm::assemble` uses (tracking
/// `org`, labels, `.align`, `.table`, `end`), pairing each word address
/// with the parallel per-line `locs` vector the backend threaded through.
/// The table is the Phase-1 debugger artifact: a breakpoint on a C line
/// resolves to the word addresses that line produced.
pub fn line_table_text(device: &Device, asm: &str, locs: &[Option<SrcLoc>]) -> String {
    let mut out = String::new();
    out.push_str(&format!("; epic-cc line table for {}\n", device.name));
    let mut org = 0usize;
    let mut li = 0usize;
    for raw in asm.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        let loc = locs.get(li).cloned().flatten();
        li += 1;
        if line.is_empty() || line.starts_with("list") || line.starts_with("radix") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            org = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
            continue;
        }
        if line.starts_with("end") {
            break;
        }
        if let Some(l) = line.strip_suffix(':') {
            // A label defines no word; the next instruction's address is
            // the label's. Skip.
            let _ = l;
            continue;
        }
        if line.contains(" equ ") {
            continue;
        }
        if let Some(n) = line.strip_prefix(".align ") {
            let n: usize = n.trim().parse().unwrap();
            org = (org + n - 1) & !(n - 1);
            continue;
        }
        if line.starts_with(".table ") {
            continue;
        }
        if let Some(loc) = loc {
            out.push_str(&format!("{} 0x{org:04X}\n", loc));
        }
        org += 1;
    }
    out
}

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
        // The driver exits on pic14e before the report runs.
        device::Core::Pic14e => unreachable!("pic14e has no backend"),
    }
}

/// The fixed region's total capacity: PIC14 common RAM, PIC18's
/// fixed_retval reservation (the access bank overlaps the GPR banks, so
/// summing it would double-count the shared window).
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
                .fixed_retval
                .expect("PIC18 devices have a fixed_retval reservation");
            hi - lo + 1
        }
        // The driver exits on pic14e before the report runs.
        device::Core::Pic14e => unreachable!("pic14e has no backend"),
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
    let ram_used: u16 = layout.bank_used.iter().sum::<u16>() + fixed_bytes(device, layout.has_isr);
    out.push_str(&format!(
        "  RAM: {ram_used}/{ram_total} bytes ({:.1}%) (overlay: a byte can be live in several frames; used = the bytes of RAM the program's allocation occupies)\n",
        ram_used as f64 * 100.0 / ram_total as f64
    ));
    for (i, &used) in layout.bank_used.iter().enumerate() {
        let (start, end) = device.ram_banks[i];
        let total = end - start + 1;
        out.push_str(&format!("    bank {i}: {used}/{total} bytes\n"));
    }
    let fixed = fixed_bytes(device, layout.has_isr);
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
