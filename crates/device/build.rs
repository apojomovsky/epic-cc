use serde::Deserialize;
use std::{env, fs, path::Path};

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct DeviceToml {
    name: String,
    core: String,
    flash_words: u32,
    ram_banks: Vec<(u16, u16)>,
    #[serde(default)]
    common_ram: Option<(u16, u16)>,
    stack_depth: u8,
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
        if !dev.flash_words.is_power_of_two() {
            panic!(
                "device: {}: flash_words {} is not a power of two; every supported \
                 part sizes program memory in powers of two, so this is a \
                 transcription error until a part proves otherwise",
                path, dev.flash_words
            );
        }
        if dev.interrupt_vectors.is_empty() {
            panic!("device: {}: interrupt_vectors must not be empty", path);
        }
        // `pic14e` keeps the single 0x0004 vector of `pic14`, so it takes the
        // same check. Having no backend is a driver-level refusal, not a
        // reason to leave its data unvalidated.
        match dev.core.as_str() {
            "pic14" | "pic14e" => {
                if dev.interrupt_vectors.len() != 1 {
                    panic!(
                        "device: {}: {} must have exactly 1 interrupt vector, got {}",
                        path,
                        dev.core,
                        dev.interrupt_vectors.len()
                    );
                }
            }
            "pic18" => {
                if dev.interrupt_vectors.len() != 2 {
                    panic!(
                        "device: {}: pic18 must have exactly 2 interrupt vectors, got {}",
                        path,
                        dev.interrupt_vectors.len()
                    );
                }
            }
            _ => panic!("device: {}: unknown core {:?}", path, dev.core),
        }
        // field validation
        for f in &dev.config.fields {
            if f.mask == 0 {
                panic!("device: {}: field {:?} mask must not be 0", path, f.name);
            }
            if f.shift >= 8 {
                panic!("device: {}: field {:?} shift must be < 8", path, f.name);
            }
            // A fuse field is a contiguous run of bits at `shift`, so width and
            // shift determine the mask. Anything else means the two disagree and
            // the resolver would place the value wrong.
            let width = f.mask.count_ones();
            let expected = (((1u16 << width) - 1) << f.shift) as u16;
            if expected > 0xFF || f.mask as u16 != expected {
                panic!(
                    "device: {}: field {:?} mask {:#04X} is not {} contiguous bit(s) \
                     at shift {} (expected {:#04X})",
                    path, f.name, f.mask, width, f.shift, expected
                );
            }
            let max_bits = if width >= 8 { 255 } else { (1u16 << width) - 1 };
            for v in &f.values {
                if (v.bits as u16) > max_bits {
                    panic!("device: {}: field {:?} value {:?} bits {} exceeds mask width {} (mask {:#04X})", path, f.name, v.name, v.bits, width, f.mask);
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
            "pub const {ident}: Device = Device {{\n    name: \"{name}\",\n    core: {core},\n    flash_words: 0x{flash:X},\n    ram_banks: &[{ram_banks}],\n    common_ram: {common},\n    stack_depth: {stack},\n    interrupt_vectors: &[{vectors}],\n    config: ConfigRegion {{\n        base_byte_addr: 0x{base:X},\n        num_bytes: {num_bytes},\n        erased_baseline: &[{erased}],\n        fields: &[\n",
            ident = ident,
            name = dev.name,
            core = core_variant,
            flash = dev.flash_words,
            ram_banks = ram_banks_str,
            common = common_str,
            stack = dev.stack_depth,
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
