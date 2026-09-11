use serde::Deserialize;
use std::{collections::HashMap, env, fs, path::Path};

include!("provenance.rs");

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct DeviceToml {
    name: String,
    core: String,
    flash_words: u32,
    ram_banks: Vec<(u16, u16)>,
    #[serde(default)]
    common_ram: Option<(u16, u16)>,
    #[serde(default)]
    access_bank: Option<(u16, u16)>,
    #[serde(default)]
    fixed_retval: Option<(u16, u16)>,
    #[serde(default)]
    isr_w_shadow: Option<u16>,
    #[serde(default)]
    isr_home_window: Option<(u16, u16)>,
    stack_depth: u8,
    #[serde(default)]
    fsr_bank_bits: u8,
    interrupt_vectors: Vec<u16>,
    config: ConfigToml,
    #[serde(default)]
    sfrs: Vec<SfrToml>,
}
#[derive(Debug, Deserialize)]
struct ConfigToml {
    base_byte_addr: u32,
    num_bytes: u16,
    erased_baseline: Vec<u8>,
    #[serde(default)]
    fields: Vec<FieldToml>,
}

#[derive(Debug, Deserialize)]
struct FieldToml {
    name: String,
    byte_offset: u16,
    mask: u8,
    shift: u8,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    locked: Option<String>,
    values: Vec<ValueToml>,
}

#[derive(Debug, Deserialize)]
struct ValueToml {
    name: String,
    bits: u8,
}

#[derive(Debug, Deserialize)]
struct SfrToml {
    name: String,
    addr: u16,
    width: u8,
    fields: Vec<SfrFieldToml>,
}

#[derive(Debug, Deserialize)]
struct SfrFieldToml {
    name: String,
    mask: u8,
    shift: u8,
}

fn const_ident(name: &str) -> String {
    // p16f877a -> PIC16F877A, p18f4550 -> PIC18F4550
    if name.starts_with('p') || name.starts_with('P') {
        format!("PIC{}", name[1..].to_ascii_uppercase())
    } else {
        name.to_ascii_uppercase()
    }
}

/// Vector-count invariants per core: one vector on the PIC14 family, two
/// (high/low priority) on PIC18, none on baseline, which has no interrupt
/// feature at all (DS41236E section 7.0).
fn check_interrupt_vectors(path: &str, core: &str, vectors: &[u16]) {
    let want = match core {
        "pic14" | "pic14e" => 1,
        "pic18" => 2,
        "pic-baseline" => 0,
        other => panic!("device: {path}: unknown core {other:?}"),
    };
    assert!(
        vectors.len() == want,
        "device: {path}: {core} must have exactly {want} interrupt vector(s), got {}",
        vectors.len()
    );
}

/// FSR carries bank-select bits on baseline and nowhere else (docs/37
/// D-2/D-3); at most two, in FSR<7:5> of the baseline ISA.
fn check_fsr_bank_bits(path: &str, core: &str, bits: u8) {
    let legal = match core {
        "pic-baseline" => bits <= 2,
        _ => bits == 0,
    };
    assert!(
        legal,
        "device: {path}: {core} cannot carry fsr_bank_bits {bits}"
    );
}

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let devices_dir = Path::new(&manifest).join("devices");
    let out_dir = env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir).join("devices.rs");

    println!("cargo:rerun-if-changed={}", devices_dir.display());

    let mut entries: Vec<(String, DeviceToml, String)> = Vec::new();

    let read_dir = fs::read_dir(&devices_dir).unwrap_or_else(|e| {
        panic!(
            "device: cannot read devices dir {}: {e}",
            devices_dir.display()
        )
    });
    for ent in read_dir {
        let ent = ent.unwrap();
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        // The directory line above only fires when a file is added or removed:
        // a directory's mtime does not change when a file's contents do.
        println!("cargo:rerun-if-changed={}", path.display());
        let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("device: cannot read {}: {e}", path.display()));
        let raw: toml::Value = content
            .parse()
            .unwrap_or_else(|e| panic!("device: parse {}: {e}", path.display()));
        if let Err(msg) = validate_provenance(path.file_name().unwrap().to_str().unwrap(), &raw) {
            panic!("{msg}");
        }
        let dev: DeviceToml = toml::from_str(&content)
            .unwrap_or_else(|e| panic!("device: parse {}: {e}", path.display()));
        // validation: file stem must match name
        if dev.name != stem {
            panic!(
                "device: file {}.toml has name {:?} but file stem is {:?}",
                path.display(),
                dev.name,
                stem
            );
        }
        entries.push((stem, dev, path.display().to_string()));
    }

    if entries.is_empty() {
        panic!(
            "device: no devices/*.toml found in {}",
            devices_dir.display()
        );
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    // validation per device
    for (stem, dev, path) in &entries {
        // ram_banks
        if dev.ram_banks.is_empty() {
            panic!("device: {}: ram_banks must not be empty", path);
        }
        let mut sorted = dev.ram_banks.clone();
        sorted.sort_by_key(|(lo, _)| *lo);
        if sorted != dev.ram_banks {
            panic!("device: {}: ram_banks must be sorted by lo", path);
        }
        for (lo, hi) in &dev.ram_banks {
            if lo > hi {
                panic!(
                    "device: {}: ram_banks entry [{:#06X},{:#06X}] lo > hi",
                    path, lo, hi
                );
            }
        }
        for i in 1..dev.ram_banks.len() {
            let prev_hi = dev.ram_banks[i - 1].1;
            let cur_lo = dev.ram_banks[i].0;
            if cur_lo <= prev_hi {
                panic!(
                    "device: {}: ram_banks overlap: [{:#06X},{:#06X}] and [{:#06X},{:#06X}]",
                    path,
                    dev.ram_banks[i - 1].0,
                    prev_hi,
                    cur_lo,
                    dev.ram_banks[i].1
                );
            }
        }
        // per-core memory map checks: common_ram is PIC14/PIC14E (and the
        // baseline shared window), access_bank + fixed_retval are PIC18
        // only. Ensures the field never carries two meanings at once
        // (epic-cc#109).
        match dev.core.as_str() {
            "pic14" | "pic14e" | "pic-baseline" => {
                // common_ram is Option, not required: a device with no
                // bank-independent byte at all (docs/39 D-2, PIC16F74's
                // shape) is a real, valid schema state -- isel's/
                // isel-pic14e's own `.expect(...)` is the enforcement point
                // for devices that actually reach codegen needing one
                // (epic-cc#393/#398; nothing else here reads common_ram as
                // Some-only).
                if dev.access_bank.is_some() {
                    panic!(
                        "device: {}: {} must not have access_bank (PIC18 only)",
                        path, dev.core
                    );
                }
                if dev.fixed_retval.is_some() {
                    panic!(
                        "device: {}: {} must not have fixed_retval (PIC18 only)",
                        path, dev.core
                    );
                }
                if let Some((clo, chi)) = dev.common_ram {
                    if clo > chi {
                        panic!(
                            "device: {}: common_ram [{:#06X},{:#06X}] lo > hi",
                            path, clo, chi
                        );
                    }
                    for (lo, hi) in &dev.ram_banks {
                        if clo <= *hi && chi >= *lo {
                            panic!("device: {}: common_ram [{:#06X},{:#06X}] overlaps ram_banks [{:#06X},{:#06X}]", path, clo, chi, lo, hi);
                        }
                    }
                }
                // isr_w_shadow/isr_home_window (docs/39 D-2): paired,
                // substituting for common_ram on a device with none.
                if dev.isr_w_shadow.is_some() != dev.isr_home_window.is_some() {
                    panic!(
                        "device: {}: isr_w_shadow and isr_home_window must be set together",
                        path
                    );
                }
                if dev.isr_w_shadow.is_some() && dev.common_ram.is_some() {
                    panic!(
                        "device: {}: isr_w_shadow is only for a device with no common_ram",
                        path
                    );
                }
                if dev.core == "pic-baseline" && dev.isr_w_shadow.is_some() {
                    panic!(
                        "device: {}: pic-baseline has no interrupts, isr_w_shadow is meaningless",
                        path
                    );
                }
                if let Some((hlo, hhi)) = dev.isr_home_window {
                    if hlo > hhi {
                        panic!(
                            "device: {}: isr_home_window [{:#06X},{:#06X}] lo > hi",
                            path, hlo, hhi
                        );
                    }
                    // isel needs 14 bytes here (scratch+retval+isr_save),
                    // the same floor ADR-034's single-region carve uses.
                    if hhi - hlo + 1 < 14 {
                        panic!("device: {}: isr_home_window [{:#06X},{:#06X}] must be at least 14 bytes (scratch + retval + ISR save area)", path, hlo, hhi);
                    }
                    // Carved OUT of ram_banks like common_ram: bank_of's
                    // addr>>7 fallback needs no ram_banks membership.
                    for (lo, hi) in &dev.ram_banks {
                        if hlo <= *hi && hhi >= *lo {
                            panic!("device: {}: isr_home_window [{:#06X},{:#06X}] overlaps ram_banks [{:#06X},{:#06X}]", path, hlo, hhi, lo, hi);
                        }
                    }
                    let w = dev.isr_w_shadow.unwrap();
                    // Proof the generator really excluded isr_w_shadow from
                    // every region, not just picked an offset and hoped.
                    for (i, (lo, hi)) in dev.ram_banks.iter().enumerate() {
                        let region_high_bits = lo & !0x7F;
                        let shadow_addr = region_high_bits | (w & 0x7F);
                        if shadow_addr >= *lo && shadow_addr <= *hi {
                            panic!("device: {}: isr_w_shadow 0x{:04X} is not excluded from ram_banks region {} [{:#06X},{:#06X}] (reconstructed address 0x{:04X} still falls inside it)", path, w, i, lo, hi, shadow_addr);
                        }
                    }
                }
            }
            "pic18" => {
                if dev.common_ram.is_some() {
                    panic!("device: {}: pic18 must not have common_ram (use access_bank + fixed_retval)", path);
                }
                let (alo, ahi) = dev
                    .access_bank
                    .unwrap_or_else(|| panic!("device: {}: pic18 requires access_bank", path));
                let (flo, fhi) = dev
                    .fixed_retval
                    .unwrap_or_else(|| panic!("device: {}: pic18 requires fixed_retval", path));
                if alo > ahi {
                    panic!(
                        "device: {}: access_bank [{:#06X},{:#06X}] lo > hi",
                        path, alo, ahi
                    );
                }
                if flo > fhi {
                    panic!(
                        "device: {}: fixed_retval [{:#06X},{:#06X}] lo > hi",
                        path, flo, fhi
                    );
                }
                if flo < alo || fhi > ahi {
                    panic!("device: {}: fixed_retval [{:#06X},{:#06X}] must lie inside access_bank [{:#06X},{:#06X}]", path, flo, fhi, alo, ahi);
                }
                // isel-pic18's compat ISR prologue/epilogue hardcodes offsets
                // up to +15 (retval backup) and +8 (W's save slot) into this
                // region (epic-cc#356); narrower than 16 bytes and those
                // writes land outside the reservation, into ordinary RAM.
                if fhi - flo + 1 < 16 {
                    panic!("device: {}: fixed_retval [{:#06X},{:#06X}] must be at least 16 bytes (retval + ISR save area)", path, flo, fhi);
                }
                for (lo, hi) in &dev.ram_banks {
                    if flo <= *hi && fhi >= *lo {
                        panic!("device: {}: fixed_retval [{:#06X},{:#06X}] overlaps ram_banks [{:#06X},{:#06X}]", path, flo, fhi, lo, hi);
                    }
                }
            }
            _ => {}
        }
        if dev.config.erased_baseline.len() != dev.config.num_bytes as usize {
            panic!(
                "device: {}: erased_baseline len {} != num_bytes {}",
                path,
                dev.config.erased_baseline.len(),
                dev.config.num_bytes
            );
        }
        if dev.flash_words == 0 {
            panic!("device: {}: flash_words must be greater than 0", path);
        }
        // isel pages CALL/GOTO in 0x800-word blocks; a partial last page
        // leaves dead space it doesn't avoid. A device under one page
        // (PIC16F84, docs/39 D-1) has no boundary to misalign.
        if matches!(dev.core.as_str(), "pic14" | "pic14e")
            && dev.flash_words > 0x800
            && dev.flash_words % 0x800 != 0
        {
            panic!(
                "device: {}: flash_words {} is not a multiple of 0x800 words, the \
                 CALL/GOTO page size pic14/pic14e's isel assumes",
                path, dev.flash_words
            );
        }
        // `pic14e` keeps the single 0x0004 vector of `pic14`, so it takes the
        // same check. Having no backend is a driver-level refusal, not a
        // reason to leave its data unvalidated.
        check_interrupt_vectors(path, &dev.core, &dev.interrupt_vectors);
        check_fsr_bank_bits(path, &dev.core, dev.fsr_bank_bits);
        // field validation
        let mut used: HashMap<u16, u8> = HashMap::new();
        for f in &dev.config.fields {
            if f.mask == 0 {
                panic!("device: {}: field {:?} mask must not be 0", path, f.name);
            }
            if f.shift >= 8 {
                panic!("device: {}: field {:?} shift must be < 8", path, f.name);
            }
            // A fuse field's mask may be scattered (PIC16F628A FOSC is 0x13:
            // bits 0, 1 and 4 of its byte, with WDTE/PWRTE in between), so
            // only the lowest set bit must equal `shift` and every value
            // must fit inside the mask once shifted. Anything else means the
            // resolver would place the value wrong.
            if f.mask.trailing_zeros() as u8 != f.shift {
                panic!(
                    "device: {}: field {:?} mask {:#04X} lowest set bit is {}, not shift {}",
                    path,
                    f.name,
                    f.mask,
                    f.mask.trailing_zeros(),
                    f.shift
                );
            }
            for v in &f.values {
                if ((v.bits as u16) << f.shift) & !(f.mask as u16) != 0 {
                    panic!("device: {}: field {:?} value {:?} bits {} outside mask {:#04X} at shift {}", path, f.name, v.name, v.bits, f.mask, f.shift);
                }
            }
            if let Some(def) = &f.default {
                if !f.values.iter().any(|v| &v.name == def) {
                    panic!(
                        "device: {}: field {:?} default {:?} not in values {:?}",
                        path,
                        f.name,
                        def,
                        f.values.iter().map(|v| &v.name).collect::<Vec<_>>()
                    );
                }
            }
            if let Some(lk) = &f.locked {
                if !f.values.iter().any(|v| &v.name == lk) {
                    panic!(
                        "device: {}: field {:?} locked {:?} not in values",
                        path, f.name, lk
                    );
                }
            }
            if f.byte_offset as usize >= dev.config.num_bytes as usize {
                panic!(
                    "device: {}: field {:?} byte_offset {} >= num_bytes {}",
                    path, f.name, f.byte_offset, dev.config.num_bytes
                );
            }
            let slot = used.entry(f.byte_offset).or_insert(0);
            if *slot & f.mask != 0 {
                panic!(
                    "device: {}: field {:?} mask {:#04X} overlaps another field in byte {}",
                    path, f.name, f.mask, f.byte_offset
                );
            }
            *slot |= f.mask;
        }
        // name check
        let _ = stem; // already validated
    }

    // codegen
    let mut out = String::new();
    out.push_str("// @generated by crates/device/build.rs from devices/*.toml -- do not edit\n");
    out.push_str(
        "// This file is included via include!(concat!(env!(\"OUT_DIR\"), \"/devices.rs\"))\n\n",
    );
    for (stem, dev, _path) in &entries {
        let ident = const_ident(stem);
        let core_variant = match dev.core.as_str() {
            "pic14" => "Core::Pic14",
            "pic18" => "Core::Pic18",
            "pic14e" => "Core::Pic14e",
            "pic-baseline" => "Core::PicBaseline",
            _ => unreachable!(),
        };
        // ram_banks
        let ram_banks_str = dev
            .ram_banks
            .iter()
            .map(|(lo, hi)| format!("(0x{lo:04X}, 0x{hi:04X})"))
            .collect::<Vec<_>>()
            .join(", ");
        let common_str = match dev.common_ram {
            Some((lo, hi)) => format!("Some((0x{lo:04X}, 0x{hi:04X}))"),
            None => "None".to_string(),
        };
        let access_str = match dev.access_bank {
            Some((lo, hi)) => format!("Some((0x{lo:04X}, 0x{hi:04X}))"),
            None => "None".to_string(),
        };
        let retval_str = match dev.fixed_retval {
            Some((lo, hi)) => format!("Some((0x{lo:04X}, 0x{hi:04X}))"),
            None => "None".to_string(),
        };
        let isr_w_shadow_str = match dev.isr_w_shadow {
            Some(w) => format!("Some(0x{w:04X})"),
            None => "None".to_string(),
        };
        let isr_home_window_str = match dev.isr_home_window {
            Some((lo, hi)) => format!("Some((0x{lo:04X}, 0x{hi:04X}))"),
            None => "None".to_string(),
        };
        let vectors_str = dev
            .interrupt_vectors
            .iter()
            .map(|v| format!("0x{v:04X}"))
            .collect::<Vec<_>>()
            .join(", ");
        let erased_str = dev
            .config
            .erased_baseline
            .iter()
            .map(|b| format!("0x{b:02X}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "pub const {ident}: Device = Device {{\n    name: \"{name}\",\n    core: {core},\n    flash_words: 0x{flash:X},\n    ram_banks: &[{ram_banks}],\n    common_ram: {common},\n    access_bank: {access},\n    fixed_retval: {retval},\n    isr_w_shadow: {isr_w_shadow},\n    isr_home_window: {isr_home_window},\n    stack_depth: {stack},\n    fsr_bank_bits: {fsb},\n    interrupt_vectors: &[{vectors}],\n    config: ConfigRegion {{\n        base_byte_addr: 0x{base:X},\n        num_bytes: {num_bytes},\n        erased_baseline: &[{erased}],\n        fields: &[\n",
            ident = ident,
            name = dev.name,
            core = core_variant,
            flash = dev.flash_words,
            ram_banks = ram_banks_str,
            common = common_str,
            access = access_str,
            retval = retval_str,
            isr_w_shadow = isr_w_shadow_str,
            isr_home_window = isr_home_window_str,
            stack = dev.stack_depth,
            fsb = dev.fsr_bank_bits,
            vectors = vectors_str,
            base = dev.config.base_byte_addr,
            num_bytes = dev.config.num_bytes,
            erased = erased_str,
        ));
        for f in &dev.config.fields {
            let default_str = match &f.default {
                Some(s) => format!("Some(\"{s}\")"),
                None => "None".to_string(),
            };
            let locked_str = match &f.locked {
                Some(s) => format!("Some(\"{s}\")"),
                None => "None".to_string(),
            };
            out.push_str(&format!(
                "            FuseField {{ name: \"{name}\", byte_offset: {off}, mask: 0x{mask:02X}, shift: {shift}, values: &[\n",
                name = f.name,
                off = f.byte_offset,
                mask = f.mask,
                shift = f.shift
            ));
            for v in &f.values {
                out.push_str(&format!(
                    "                FuseValue {{ name: \"{name}\", bits: {bits} }},\n",
                    name = v.name,
                    bits = v.bits
                ));
            }
            out.push_str(&format!(
                "            ], default: {default}, locked: {locked} }},\n",
                default = default_str,
                locked = locked_str
            ));
        }
        out.push_str("        ],\n    },\n    sfrs: &[\n");
        for s in &dev.sfrs {
            out.push_str(&format!(
                "        Sfr {{ name: \"{name}\", addr: 0x{addr:04X}, width: {width}, fields: &[\n",
                name = s.name,
                addr = s.addr,
                width = s.width
            ));
            for f in &s.fields {
                out.push_str(&format!(
                    "            SfrField {{ name: \"{name}\", mask: 0x{mask:02X}, shift: {shift} }},\n",
                    name = f.name,
                    mask = f.mask,
                    shift = f.shift
                ));
            }
            out.push_str("        ] },\n");
        }
        out.push_str("    ],\n};\n\n");
    }
    // ALL and by_name
    let idents: Vec<String> = entries
        .iter()
        .map(|(stem, _, _)| const_ident(stem))
        .collect();
    let refs = idents
        .iter()
        .map(|id| format!("&{id}"))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("pub const ALL: &[&Device] = &[{}];\n\n", refs));
    out.push_str("pub fn by_name(name: &str) -> Option<&'static Device> {\n    match name {\n");
    for (stem, _, _) in &entries {
        let ident = const_ident(stem);
        out.push_str(&format!(
            "        \"{stem}\" => Some(&{ident}),\n",
            stem = stem,
            ident = ident
        ));
    }
    out.push_str("        _ => None,\n    }\n}\n");
    // also case-insensitive helper used by driver
    out.push_str("\npub fn by_name_case_insensitive(name: &str) -> Option<&'static Device> {\n    let lower = name.to_ascii_lowercase();\n    by_name(&lower)\n}\n");

    fs::write(&out_path, out)
        .unwrap_or_else(|e| panic!("device: cannot write {}: {e}", out_path.display()));
}
