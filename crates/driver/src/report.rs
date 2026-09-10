//! The size report the driver prints to stderr after every hex build.
//!
//! "RAM used" is the bytes of RAM the program's allocation occupies: the
//! per-bank high-water marks from the overlay layout plus the fixed
//! scratch/retval/ISR-save region isel reserves. Overlay allocation makes
//! this less obvious than on a stack machine, since a byte can be live in
//! several frames, so the report states the definition on the line.

use super::sidecar;
use alloc::AllocLayout;
use device::Device;
use ir::SrcLoc;
use irparse::DebugVars;

/// The address-to-source-line table: one `file:line:col <addr>` record per
/// word of the final program, sorted by address. Compiler-generated words
/// (no source instruction) are omitted. Builds by walking the final asm
/// text with the same pass-1 semantics `asm::assemble` uses (tracking
/// `org`, labels, `.align`, `.table`, `end`), pairing each word address
/// with the parallel per-line `locs` vector the backend threads through.
/// The table is the debugger artifact: a breakpoint on a C line
/// resolves to the word addresses that line produced.
pub fn line_table_text(device: &Device, asm: &str, locs: &[Option<SrcLoc>]) -> String {
    let mut out = String::new();
    out.push_str(&format!("; epic-cc line table for {}\n", device.name));
    // The rows are the sidecar's `.debug_line` source by construction:
    // both artifacts walk out of `sidecar::line_rows`, so they cannot
    // drift apart.
    for (loc, addr) in sidecar::line_rows(asm, locs) {
        out.push_str(&format!("{loc} 0x{addr:04X}\n"));
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

/// The typed variable table: `global <name> 0xNN TYPE` and
/// `local {func}::{name} 0xNN TYPE`, one flattened record per variable
/// the join maps to an address. The join reads `DebugVars` metadata
/// against `AllocLayout`: globals by C name, locals through the SSA key
/// their `#dbg_*` record names. A variable with no allocation (promoted,
/// constant-folded, or otherwise optimized away) stays omitted; the TYPE
/// field is the flat debug print, the in-process table stays the DWARF source.
pub fn var_table_text(device: &Device, layout: &AllocLayout, vars: &DebugVars) -> String {
    let mut out = String::new();
    out.push_str(&format!("; epic-cc var table for {}\n", device.name));
    for v in &vars.globals {
        if let Some(func) = &v.func {
            // Function-local static: clang names the symbol
            // `{func}.{name}`, which is the map's global key.
            if let Some(&addr) = layout.globals.get(&format!("{func}.{name}", name = v.name)) {
                out.push_str(&format!(
                    "local {func}::{name} 0x{addr:02X} {ty}\n",
                    name = v.name,
                    ty = vars.type_string(v.ty)
                ));
            }
        } else if let Some(&addr) = layout.globals.get(&v.name) {
            out.push_str(&format!(
                "global {name} 0x{addr:02X} {ty}\n",
                name = v.name,
                ty = vars.type_string(v.ty)
            ));
        }
    }
    for v in &vars.locals {
        let Some(func) = &v.func else { continue };
        let Some(ssa) = &v.ssa else { continue };
        if let Some(&addr) = layout.locals.get(&format!("{func}::{ssa}")) {
            out.push_str(&format!(
                "local {func}::{name} 0x{addr:02X} {ty}\n",
                name = v.name,
                ty = vars.type_string(v.ty)
            ));
        }
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
        device::Core::Pic14 | device::Core::Pic14e => {
            let base = 1 + 4; // scratch + retval
            if has_isr {
                // The ISR save area (W/STATUS/PCLATH/FSR/retval x4/scratch
                // = 9 bytes) sits right after the retval region.
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
        device::Core::PicBaseline => {
            // Baseline's fixed region is common RAM (0x07-0x0F): scratch
            // (1) + retval (4) + scratch2 (1, the ADDLW-replacement temp)
            // + store_tmp (1, the indirect-store staging byte) = 7 bytes,
            // no ISR save (no interrupts).
            let base = 7;
            if has_isr {
                panic!("report: baseline has no interrupts; has_isr must be false")
            } else {
                base
            }
        }
    }
}

/// The fixed region's total capacity: PIC14/PIC14E common RAM, PIC18's
/// fixed_retval reservation (the access bank overlaps the GPR banks, so
/// summing it would double-count the shared window).
pub fn fixed_total(device: &Device) -> u16 {
    match device.core {
        device::Core::Pic14 | device::Core::Pic14e => {
            let (lo, hi) = device
                .common_ram
                .expect("PIC14/PIC14E devices have a common-RAM region");
            hi - lo + 1
        }
        device::Core::Pic18 => {
            let (lo, hi) = device
                .fixed_retval
                .expect("PIC18 devices have a fixed_retval reservation");
            hi - lo + 1
        }
        device::Core::PicBaseline => {
            let (lo, hi) = device
                .common_ram
                .expect("baseline devices have a common-RAM region");
            hi - lo + 1
        }
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
        device::Core::PicBaseline => "common",
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
