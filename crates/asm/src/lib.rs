//! Two-pass PIC14 (PIC16F877A) assembler to 14-bit words and Intel HEX.
//!
//! No external assembler: `assemble` resolves labels/org/equ in a first pass and
//! encodes instructions in a second pass; `to_hex` emits Intel HEX in the exact
//! byte order decoded by `pic14_sim::parse_hex` (two little-endian bytes per
//! 14-bit word at `word*2`, `04` extended-linear-address header, `01` EOF).

/// Assemble PIC14 assembly source into 14-bit words indexed by word address.
pub fn assemble(src: &str) -> Vec<u16> {
    let mut symbols: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut org = 0usize;
    // Pass 1: labels, org, equ; measure size.
    let mut lines: Vec<(usize, String)> = Vec::new(); // (address, mnemonic line)
    for raw in src.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("list") || line.starts_with("radix") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            org = parse_num(rest.trim());
            continue;
        }
        if let Some(_rest) = line.strip_prefix("end") {
            break;
        }
        if let Some(label) = line.strip_suffix(':') {
            symbols.insert(label.trim().to_string(), org);
            continue;
        }
        if let Some(eq) = line.find(" equ ") {
            let (name, val) = line.split_at(eq);
            symbols.insert(name.trim().to_string(), parse_num(val[" equ ".len()..].trim()));
            continue;
        }
        lines.push((org, line.to_string()));
        org += 1;
    }
    // Pass 2: encode.
    let mut out = vec![0u16; org];
    for (addr, line) in &lines {
        out[*addr] = encode(line, &symbols);
    }
    out
}

/// Assemble source and render the result as Intel HEX.
pub fn assemble_file_to_hex(src: &str) -> String {
    to_hex(&assemble(src))
}

fn parse_num(s: &str) -> usize {
    if let Some(h) = s.strip_prefix("0x") {
        usize::from_str_radix(h, 16).unwrap()
    } else {
        s.parse().unwrap()
    }
}

fn encode(line: &str, sym: &std::collections::HashMap<String, usize>) -> u16 {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let mne = parts[0].to_ascii_uppercase();
    let op = parts.get(1).copied().unwrap_or("");
    let f = |s: &str| -> u16 { parse_num(s.trim_end_matches(',')) as u16 & 0x7F };
    match mne.as_str() {
        "NOP" => 0x0000,
        "RETURN" => 0x0008,
        "SLEEP" => 0x0063,
        "CLRWDT" => 0x0064,
        "MOVWF" => 0x0080 | f(op),
        "CLRF" => 0x0180 | f(op),
        "MOVF" => 0x0800 | f(op),
        "ADDWF" => 0x0700 | f(op),
        "SUBWF" => 0x0200 | f(op),
        "ANDWF" => 0x0500 | f(op),
        "IORWF" => 0x0400 | f(op),
        "XORWF" => 0x0600 | f(op),
        "MOVLW" => 0x3000 | parse_num(op) as u16,
        "ADDLW" => 0x3E00 | parse_num(op) as u16,
        "ANDLW" => 0x3900 | parse_num(op) as u16,
        "IORLW" => 0x3800 | parse_num(op) as u16,
        "XORLW" => 0x3A00 | parse_num(op) as u16,
        "SUBLW" => 0x3C00 | parse_num(op) as u16,
        "RETLW" => 0x3400 | parse_num(op) as u16,
        "BTFSC" | "BTFSS" | "BCF" | "BSF" => {
            let (freg, b) = op.split_once(',').unwrap();
            let base = match mne.as_str() {
                "BTFSC" => 0x1800,
                "BTFSS" => 0x1C00,
                "BCF" => 0x1000,
                _ => 0x1400,
            };
            base | ((parse_num(b.trim()) as u16 & 7) << 7) | f(freg.trim())
        }
        "GOTO" => 0x2800 | (sym.get(op).copied().unwrap_or_else(|| parse_num(op)) as u16 & 0x7FF),
        "CALL" => 0x2000 | (sym.get(op).copied().unwrap_or_else(|| parse_num(op)) as u16 & 0x7FF),
        other => panic!("asm: unsupported mnemonic {other}"),
    }
}

/// Intel HEX from 14-bit words: little-endian pairs at word*2.
pub fn to_hex(words: &[u16]) -> String {
    let mut hex = String::new();
    // gpasm emits a leading 04 extended-linear-address record (upper 16 bits = 0).
    hex.push_str(":020000040000FA\n");
    // trim trailing zeros to the highest set word
    let hi = words.iter().rposition(|&w| w != 0).map(|i| i + 1).unwrap_or(0);
    let mut addr = 0usize;
    while addr < hi {
        // gpasm chunks at 16 data bytes (8 words) per record.
        let n = (hi - addr).min(8);
        let mut body = vec![0u8; 2 * n];
        for (i, w) in words[addr..addr + n].iter().enumerate() {
            body[2 * i] = (w & 0xFF) as u8;
            body[2 * i + 1] = ((w >> 8) & 0xFF) as u8;
        }
        let byte_addr = addr * 2;
        let mut rec = vec![(2 * n) as u8, (byte_addr >> 8) as u8, (byte_addr & 0xFF) as u8, 0x00];
        rec.extend_from_slice(&body);
        let sum: u16 = rec.iter().map(|&b| b as u16).sum();
        rec.push((0x100 - (sum & 0xFF)) as u8);
        hex.push_str(":");
        for b in &rec {
            hex.push_str(&format!("{b:02X}"));
        }
        hex.push('\n');
        addr += n;
    }
    hex.push_str(":00000001FF\n");
    hex
}
