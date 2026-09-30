//! The size report the driver prints to stderr after every hex build.
//!
//! "RAM used" is the bytes of RAM the program's allocation occupies: the
//! per-bank high-water marks from the overlay layout plus the fixed
//! scratch/retval/ISR-save bytes the emitted code can touch. The fixed
//! part counts use, not reservation: a program with no calls leaves the
//! retval region idle, like the ISR save area with no ISR. Overlay
//! allocation makes this less obvious than on a stack machine, since a
//! byte can be live in several frames, so the report states the
//! definition on the line.

use super::sidecar;
use alloc::AllocLayout;
use device::Device;
use ir::{BinOp, Inst, MemLen, Module, SrcLoc, Val};
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
/// RAM address), `staged <name>` (shared-buffer staging, epic-cc#790),
/// `local <key> 0xNN` where `<key>` is the driver's `{func}::{name}`
/// HashMap key, all sorted deterministically, then the alloc scalars the
/// e2e tests assert on: `total-bank0`, per-bank `bank-used <i>`, and the
/// ISR save bases present on that program (`isr-low-save`, `isr-save`,
/// `isr-hi-save`, epic-cc#814). The map is the artifact a user reads when
/// a program does not fit and they have to decide what to cut.
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
    let mut staged: Vec<&String> = layout.staged_consts.iter().collect();
    staged.sort();
    for name in staged {
        out.push_str(&format!("staged {name}\n"));
    }
    let mut locals: Vec<&String> = layout.locals.keys().collect();
    locals.sort();
    for key in locals {
        out.push_str(&format!("local {key} 0x{:02X}\n", layout.locals[key]));
    }
    out.push_str(&format!("total-bank0 0x{:02X}\n", layout.total_bank0));
    for (i, &used) in layout.bank_used.iter().enumerate() {
        out.push_str(&format!("bank-used {i} 0x{used:02X}\n"));
    }
    for (kind, slot) in [
        ("isr-low-save", layout.isr_low_save),
        ("isr-save", layout.isr_save),
        ("isr-hi-save", layout.isr_hi_save),
    ] {
        if let Some(addr) = slot {
            out.push_str(&format!("{kind} 0x{addr:02X}\n"));
        }
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

/// How much of the fixed region the emitted program can touch.
/// `retval_bytes` is the widest touched retval byte count; `flag` is the
/// PIC18 borrow-chain spill bit in retval byte 0 (it shares the byte, so
/// it only ever raises the count to 1). PIC14 scratch stays out: it is
/// always counted, its uses are too broad to mirror cheaply.
pub struct FixedUses {
    pub retval_bytes: u8,
    pub flag: bool,
}

/// The fixed bytes the module's lowering can touch, per core. Every arm
/// mirrors an isel emission site, so a site missing here undercounts RAM:
/// the `fixed_uses` e2e test scans emitted asm for fixed-range refs over
/// the fixture corpus and fails any row the scan beats. Verbatim inline
/// asm is outside the model: it can name any byte, fixed or otherwise.
/// Each predicate fires on shapes that MIGHT touch, so misses default
/// to counted.
///
/// PIC18 (isel-pic18): valued calls load `retval_lo..`, `Ret` stores the
/// same width, the `k - a` chain parks C0 in the flag bit, `_delay`
/// counts in `retval_lo..`. Mul/div/float bodies write their own result,
/// covered by their return. i64 never reaches a backend (irparse rejects
/// it), so widths above 4 cap in `fixed_bytes`.
///
/// PIC14/PIC14E (isel, isel-pic14e): same call/return widths, plus the
/// dynamic-memcpy counters (2, constant lengths unroll), the `_delay`
/// counters, the large-const-table index (1, tables over 255 bytes), and
/// the signed-wide-compare spill (1, unsigned chains fold in place).
pub fn fixed_uses(module: &Module, core: device::Core) -> FixedUses {
    let mut retval: u8 = 0;
    let mut flag = false;
    let mut delay: u8 = 0;
    let mut memcpy = false;
    let mut big_table = false;
    let mut wide_icmp = false;
    for f in &module.funcs {
        if let Some(t) = f.ret {
            retval = retval.max(t.bytes());
        }
        for b in &f.blocks {
            for inst in &b.insts {
                match inst {
                    Inst::Ret(Some((t, _)), _) => {
                        retval = retval.max(t.bytes());
                    }
                    Inst::Call(c) => {
                        if let Some(t) = c.ty {
                            retval = retval.max(t.bytes());
                        }
                        if c.func == "_delay" && c.callees.is_empty() {
                            match c.args.as_slice() {
                                [a] => match a.val {
                                    Val::Const(n) if n >= 0 => {
                                        let plan = iselcore::delay::plan_delay(n as u64);
                                        let d = plan.nests.iter().map(Vec::len).max().unwrap_or(0)
                                            as u8;
                                        delay = delay.max(d);
                                    }
                                    // isel rejects these shapes, so the
                                    // build fails before any report prints.
                                    _ => delay = delay.max(3),
                                },
                                _ => delay = delay.max(3),
                            }
                        } else if c.func == "_delay" {
                            delay = delay.max(3);
                        }
                    }
                    Inst::Bin(bin) => {
                        if core == device::Core::Pic18
                            && bin.op == BinOp::Sub
                            && bin.ty.bytes() > 1
                            && matches!(bin.a, Val::Const(_))
                        {
                            flag = true;
                        }
                    }
                    Inst::Memcpy(m) => {
                        // A constant length unrolls per byte with no
                        // counters; only the dynamic loop borrows them.
                        if matches!(m.len, MemLen::Reg(_)) {
                            memcpy = true;
                        }
                    }
                    Inst::Icmp(ic) => {
                        // Only the signed high byte spills into the temp;
                        // unsigned wide chains fold in place.
                        if ic.ty.bytes() > 1
                            && matches!(ic.pred.as_str(), "slt" | "sle" | "sgt" | "sge")
                        {
                            wide_icmp = true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if module.globals.iter().any(|g| g.is_const && g.size > 255) {
        big_table = true;
    }
    match core {
        device::Core::Pic18 => FixedUses {
            retval_bytes: retval.max(delay),
            flag,
        },
        device::Core::Pic14 | device::Core::Pic14e => {
            let mut r = retval.max(delay);
            if memcpy {
                r = r.max(2);
            }
            if big_table || wide_icmp {
                r = r.max(1);
            }
            FixedUses {
                retval_bytes: r,
                flag: false,
            }
        }
        device::Core::PicBaseline => FixedUses {
            retval_bytes: 4,
            flag: true,
        },
    }
}

/// The fixed bytes the program occupies outside the overlay: PIC14's
/// common-RAM scratch (1, always) + the touched retval bytes from
/// `fixed_uses`, plus the ISR save area (9) when the program has an ISR.
/// PIC18's touched retval/flag bytes (the flag shares byte 0), plus the
/// ISR save area (12) when present. Layout constants live in isel
/// (crates/isel/src/lib.rs, crates/isel-pic18/src/lib.rs).
pub fn fixed_bytes(device: &Device, has_isr: bool, uses: &FixedUses) -> u16 {
    match device.core {
        device::Core::Pic14 | device::Core::Pic14e => {
            let base = 1 + u16::from(uses.retval_bytes.min(4)); // scratch + retval
            if has_isr {
                // The ISR save area (W/STATUS/PCLATH/FSR/retval x4/scratch
                // = 9 bytes) sits right after the retval region.
                base + 9
            } else {
                base
            }
        }
        device::Core::Pic18 => {
            // The flag bit lives in retval byte 0, so it only raises an
            // otherwise empty count to 1.
            let base = u16::from(uses.retval_bytes.min(4)).max(u16::from(uses.flag));
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
            // docs/39 D-2 (epic-cc#393): a device with no common-RAM
            // region at all substitutes isr_home_window instead.
            let (lo, hi) = device
                .common_ram
                .or(device.isr_home_window)
                .expect("PIC14/PIC14E devices have a common-RAM region or an isr_home_window");
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

/// RAM `(used, total)` in bytes: every GPR bank plus the fixed region, the
/// same definition the size report states on its RAM line.
pub fn ram_usage(device: &Device, layout: &AllocLayout, uses: &FixedUses) -> (u16, u16) {
    let total = device
        .ram_banks
        .iter()
        .map(|&(s, e)| e - s + 1)
        .sum::<u16>()
        + fixed_total(device);
    let used = layout.bank_used.iter().sum::<u16>() + fixed_bytes(device, layout.has_isr, uses);
    (used, total)
}

/// Each config field's value in `bytes`, by canonical name; `None` when the
/// bits match no named value. A scattered mask (628A `osc`, 0x13) cannot be
/// inverted with a shift, so values are matched by placing their bits the
/// way `resolve_config` does.
pub fn decode_config<'a>(
    region: &'a device::ConfigRegion,
    bytes: &[u8],
) -> Vec<(&'a str, Option<&'a str>)> {
    region
        .fields
        .iter()
        .map(|f| {
            let have = bytes[f.byte_offset as usize] & f.mask;
            let value = f
                .values
                .iter()
                .find(|v| (v.bits << f.shift) & f.mask == have)
                .map(|v| v.name);
            (f.name, value)
        })
        .collect()
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The `--report` file: the build's facts as JSON for tools that act on
/// them (the PlatformIO size bar and pre-flash checks, docs/46 D-7, D-9).
/// The format is a contract; ADR-025 defines every key and null.
pub fn report_json(
    device: &Device,
    layout: &AllocLayout,
    uses: &FixedUses,
    flash_used: usize,
    config: Option<&[u8]>,
    clock_hz: u64,
) -> String {
    let (ram_used, ram_total) = ram_usage(device, layout, uses);
    // With no configuration the HEX carries no config words, so the part
    // keeps its erased state. The baseline is that state on PIC14-family
    // parts; on PIC18 it is gpasm's all-ones fill, not the silicon default
    // (DS39632E Table 25-1 has zeros), so nothing is claimed there.
    let (source, bytes) = match (config, device.core) {
        (Some(b), _) => ("program", Some(b)),
        (None, device::Core::Pic18) => ("unset", None),
        (None, _) => ("erased", Some(device.config.erased_baseline)),
    };
    let core = match device.core {
        device::Core::Pic14 => "pic14",
        device::Core::Pic14e => "pic14e",
        device::Core::Pic18 => "pic18",
        device::Core::PicBaseline => "pic-baseline",
    };
    let clock = if clock_hz == 0 {
        "null".to_string()
    } else {
        clock_hz.to_string()
    };
    let byte_list = bytes.map_or("null".to_string(), |b| {
        let items: Vec<String> = b.iter().map(|x| x.to_string()).collect();
        format!("[{}]", items.join(", "))
    });
    let fields: Vec<String> = device
        .config
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let value = bytes.and_then(|b| decode_config(&device.config, b)[i].1);
            let v = value.map_or("null".to_string(), json_str);
            format!("      {}: {v}", json_str(f.name))
        })
        .collect();
    format!(
        "{{\n  \"version\": 1,\n  \"device\": {},\n  \"core\": \"{core}\",\n  \"flash_words\": {{ \"used\": {flash_used}, \"total\": {} }},\n  \"ram_bytes\": {{ \"used\": {ram_used}, \"total\": {ram_total} }},\n  \"clock_hz\": {clock},\n  \"config\": {{\n    \"source\": \"{source}\",\n    \"base_byte_addr\": {},\n    \"bytes\": {byte_list},\n    \"fields\": {{\n{}\n    }}\n  }}\n}}\n",
        json_str(device.name),
        device.flash_words,
        device.config.base_byte_addr,
        fields.join(",\n"),
    )
}

/// Render the size report. `flash_used` is the program's assembled word
/// count (before config-word insertion); `layout` carries the RAM facts.
pub fn render_size(
    device: &Device,
    layout: &AllocLayout,
    uses: &FixedUses,
    flash_used: usize,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("epic-cc: program size for {}:\n", device.name));
    out.push_str(&format!(
        "  flash: {flash_used}/{} words ({:.1}%)\n",
        device.flash_words,
        flash_used as f64 * 100.0 / device.flash_words as f64
    ));
    let (ram_used, ram_total) = ram_usage(device, layout, uses);
    out.push_str(&format!(
        "  RAM: {ram_used}/{ram_total} bytes ({:.1}%) (overlay: a byte can be live in several frames; used = the bytes of RAM the program's allocation occupies)\n",
        ram_used as f64 * 100.0 / ram_total as f64
    ));
    for (i, &used) in layout.bank_used.iter().enumerate() {
        let (start, end) = device.ram_banks[i];
        let total = end - start + 1;
        out.push_str(&format!("    bank {i}: {used}/{total} bytes\n"));
    }
    let fixed = fixed_bytes(device, layout.has_isr, uses);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ir::{Bin, Block, Call, CallArg, Func, Global, Icmp, MemLen, Memcpy, Ty};

    fn void_main(insts: Vec<Inst>) -> Module {
        Module {
            globals: vec![],
            funcs: vec![Func {
                name: "main".to_string(),
                ret: None,
                params: vec![],
                blocks: vec![Block {
                    label: "entry".to_string(),
                    insts,
                }],
                isr: false,
                irq_priority: 0,
                naked: false,
                variadic: false,
            }],
            module_asm: vec![],
        }
    }

    fn pic18() -> device::Core {
        device::Core::Pic18
    }

    fn pic14() -> device::Core {
        device::Core::Pic14
    }

    #[test]
    fn empty_program_touches_no_fixed_bytes() {
        let m = void_main(vec![]);
        let u18 = fixed_uses(&m, pic18());
        assert_eq!((u18.retval_bytes, u18.flag), (0, false));
        let u14 = fixed_uses(&m, pic14());
        assert_eq!((u14.retval_bytes, u14.flag), (0, false));
    }

    #[test]
    fn call_and_return_widths_size_retval() {
        let call = Inst::Call(Call {
            dst: Some("r".to_string()),
            ty: Some(Ty::I16),
            func: "f".to_string(),
            args: vec![],
            callees: vec![],
            loc: None,
        });
        let ret = Inst::Ret(Some((Ty::I16, Val::Reg("r".to_string()))), None);
        let m = void_main(vec![call, ret]);
        assert_eq!(fixed_uses(&m, pic18()).retval_bytes, 2);
        assert_eq!(fixed_uses(&m, pic14()).retval_bytes, 2);
    }

    #[test]
    fn wide_const_sub_sets_pic18_flag_only() {
        let sub = Inst::Bin(Bin {
            dst: "r".to_string(),
            op: BinOp::Sub,
            ty: Ty::I16,
            a: Val::Const(7),
            b: Val::Reg("x".to_string()),
            loc: None,
        });
        let m = void_main(vec![sub]);
        assert!(fixed_uses(&m, pic18()).flag);
        assert_eq!(fixed_uses(&m, pic18()).retval_bytes, 0);
        assert!(!fixed_uses(&m, pic14()).flag);
    }

    #[test]
    fn narrow_const_sub_sets_no_flag() {
        let sub = Inst::Bin(Bin {
            dst: "r".to_string(),
            op: BinOp::Sub,
            ty: Ty::I8,
            a: Val::Const(7),
            b: Val::Reg("x".to_string()),
            loc: None,
        });
        assert!(!fixed_uses(&void_main(vec![sub]), pic18()).flag);
    }

    #[test]
    fn dynamic_memcpy_touches_pic14_retval() {
        let reg = Inst::Memcpy(Memcpy {
            dst: Val::Reg("d".to_string()),
            src: Val::Reg("s".to_string()),
            len: MemLen::Reg(Val::Reg("n".to_string())),
            loc: None,
        });
        assert_eq!(fixed_uses(&void_main(vec![reg]), pic14()).retval_bytes, 2);
        let fixed = Inst::Memcpy(Memcpy {
            dst: Val::Reg("d".to_string()),
            src: Val::Reg("s".to_string()),
            len: MemLen::Const(4),
            loc: None,
        });
        assert_eq!(fixed_uses(&void_main(vec![fixed]), pic14()).retval_bytes, 0);
    }

    #[test]
    fn signed_wide_icmp_touches_pic14_retval() {
        let cmp = Inst::Icmp(Icmp {
            dst: "c".to_string(),
            pred: "slt".to_string(),
            ty: Ty::I16,
            a: Val::Reg("x".to_string()),
            b: Val::Reg("y".to_string()),
            loc: None,
        });
        assert_eq!(
            fixed_uses(&void_main(vec![cmp.clone()]), pic14()).retval_bytes,
            1
        );
        assert_eq!(fixed_uses(&void_main(vec![cmp]), pic18()).retval_bytes, 0);
        let unsigned = Inst::Icmp(Icmp {
            dst: "c".to_string(),
            pred: "ult".to_string(),
            ty: Ty::I16,
            a: Val::Reg("x".to_string()),
            b: Val::Reg("y".to_string()),
            loc: None,
        });
        assert_eq!(
            fixed_uses(&void_main(vec![unsigned]), pic14()).retval_bytes,
            0
        );
    }

    #[test]
    fn big_const_table_touches_pic14_retval() {
        let m = Module {
            globals: vec![Global {
                name: "tab".to_string(),
                ty: Ty::I8,
                is_const: true,
                size: 300,
                bytes: vec![],
                refs: vec![],
                addr: None,
            }],
            funcs: vec![],
            module_asm: vec![],
        };
        assert_eq!(fixed_uses(&m, pic14()).retval_bytes, 1);
        assert_eq!(fixed_uses(&m, pic18()).retval_bytes, 0);
    }

    #[test]
    fn const_delay_counts_its_nests() {
        let delay = Inst::Call(Call {
            dst: None,
            ty: None,
            func: "_delay".to_string(),
            args: vec![CallArg {
                ty: Some(Ty::I16),
                val: Val::Const(1000),
                byval: None,
                sret: false,
            }],
            callees: vec![],
            loc: None,
        });
        let m = void_main(vec![delay]);
        let plan = iselcore::delay::plan_delay(1000);
        let depth = plan.nests.iter().map(Vec::len).max().unwrap_or(0) as u8;
        assert_eq!(fixed_uses(&m, pic18()).retval_bytes, depth);
        assert_eq!(fixed_uses(&m, pic14()).retval_bytes, depth);
    }
}
