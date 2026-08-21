//! Two-pass PIC14 assembler to 14-bit words and Intel HEX.
//!
//! No external assembler: `assemble` resolves labels/org/equ in a first pass and
//! encodes instructions in a second pass; `to_hex` emits Intel HEX in the exact
//! byte order decoded by `pic14_sim::parse_hex` (two little-endian bytes per
//! 14-bit word at `word*2`, `04` extended-linear-address header, `01` EOF).

use device::Device;

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
            let target = parse_num(rest.trim());
            // `.org` can only pad FORWARD (the isel page pads and the pinned
            // table-section start are always at or ahead of the running
            // address). A backward `.org` would overwrite already-emitted
            // words, silently relocating code — a post-layout drift (e.g. a
            // banking pass inserting words) pushing a page base backwards
            // would otherwise go unnoticed, so it must fail loudly.
            assert!(
                target >= org,
                "asm: backward .org to 0x{target:04X} from 0x{org:04X} — an .org can only pad forward; a backward target would overwrite emitted words"
            );
            org = target;
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
        // `.align N`: pad with NOP words (zeros) to the next N-word
        // boundary — isel emits `.align 256` before a chunked (> 255 byte)
        // const table's base label so LOW(base) == 0 and chunk 1, emitted
        // immediately after chunk 0, also sits at LOW == 0.
        if let Some(n) = line.strip_prefix(".align ") {
            let n = parse_num(n.trim());
            assert!(
                n >= 2 && n.is_power_of_two(),
                "asm: .align needs a power-of-two word count, got {n}"
            );
            org = (org + n - 1) & !(n - 1);
            continue;
        }
        // `.table <name> <size>`: emitted immediately before a const table's
        // base label; `org` here IS the base address (labels take no words).
        // The computed `ADDLW LOW(base); MOVWF PCL` jump wraps within the
        // 256-byte window selected by the reader's PCLATH set, so the whole
        // table must fit one window — accepting one that doesn't (reads past
        // the boundary return the wrong window's bytes) is the exact
        // miscompile this directive exists to prevent, so we panic loudly.
        if let Some(rest) = line.strip_prefix(".table ") {
            let mut it = rest.split_whitespace();
            let name = it.next().expect("asm: .table needs a table name");
            let size = parse_num(it.next().expect("asm: .table needs a table size"));
            let lo = org & 0xFF;
            if size <= 255 {
                assert!(
                    lo + size <= 0x100,
                    "asm: const table {name} of {size} bytes at base 0x{org:03X} crosses its 256-byte window (LOW 0x{lo:02X} + {size} > 0x100) — reads past the window would silently wrap"
                );
            } else {
                assert!(
                    lo == 0,
                    "asm: const table {name} of {size} bytes must be 256-aligned (base 0x{org:03X}, LOW 0x{lo:02X}) — a chunked table's chunks must sit at LOW == 0 or reads silently wrap"
                );
            }
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

/// Word count of one PIC18 instruction line's mnemonic: 2 for the
/// two-word forms (`GOTO`/`CALL`/`LFSR`/`MOVFF`), 1 for everything else.
/// `DB` never reaches this: `assemble_pic18` handles it in pass 1 (one
/// byte per `db` value, packed two per word, see the `DB` doc there).
fn instruction_words_pic18(line: &str) -> usize {
    let mne = line.split_whitespace().next().unwrap_or("");
    match mne.to_ascii_uppercase().as_str() {
        "GOTO" | "CALL" | "LFSR" | "MOVFF" => 2,
        _ => 1,
    }
}

/// Assemble PIC18 assembly source into 16-bit words indexed by word
/// address. Two-pass like `assemble`: pass 1 resolves labels/`org`/`equ`
/// and measures each line's size; pass 2 encodes. `DB <hex> [, <hex>]*`
/// (case-insensitive) is a byte-data directive packing bytes into words
/// little-endian, two per word (even byte = low, odd byte = high, the
/// byte-packed flash model `TBLRD` reads, matching `gpasm -p p18f4550`'s
/// own `DB` packing): each `db` value advances `org` by ONE byte, and pass
/// 2 ORs each byte into `out[addr/2]` at shift `(addr % 2) * 8`. `DB` is
/// not an instruction, so it never reaches `encode_pic18`.
///
/// **`org` and labels are BYTE addresses here, unlike PIC14's `assemble`**
/// (confirmed against `gpasm -p p18f4550`: `org 0x0020` places the next
/// instruction at *word* address 0x10, not 0x20) — this matches PIC18's
/// byte-oriented program counter. The output `Vec<u16>` stays word-indexed
/// (byte address / 2), so callers see the same shape as `assemble`;
/// `encode_pic18` receives each instruction's own BYTE address and divides
/// by 2 wherever the ISA's `k`/`n` fields need a *word* address/offset
/// (`GOTO`/`CALL`'s absolute target, every relative branch's offset).
pub fn assemble_pic18(src: &str) -> Vec<u16> {
    let mut symbols: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut org = 0usize; // byte address
    let mut lines: Vec<(usize, String)> = Vec::new(); // (byte address, line)
    let mut db_bytes: Vec<(usize, u8)> = Vec::new(); // (byte address, value)
    for raw in src.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("list") || line.starts_with("radix") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            let target = parse_num(rest.trim());
            assert!(
                target >= org,
                "asm: backward .org to 0x{target:04X} from 0x{org:04X} — an .org can only pad forward"
            );
            org = target;
            continue;
        }
        if line.strip_prefix("end").is_some() {
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
        if line.to_ascii_lowercase().starts_with("db ") {
            for tok in line[3..].split(',') {
                let s = tok.trim();
                if s.is_empty() {
                    continue;
                }
                db_bytes.push((org, (parse_num(s) & 0xFF) as u8));
                org += 1; // one byte per db value
            }
            continue;
        }
        let words = instruction_words_pic18(line);
        lines.push((org, line.to_string()));
        org += words * 2; // advance by BYTES
    }
    let mut out = vec![0u16; (org + 1) / 2];
    for (addr, line) in &lines {
        let word_addr = addr / 2;
        let encoded = encode_pic18(*addr, line, &symbols);
        out[word_addr] = encoded[0];
        if encoded.len() == 2 {
            out[word_addr + 1] = encoded[1];
        }
    }
    for (addr, b) in db_bytes {
        out[addr / 2] |= (u16::from(b)) << ((addr % 2) * 8);
    }
    out
}

/// Parse a PIC18 byte/bit-oriented operand's `f` field (the first
/// comma-separated token after the mnemonic).
fn parse_f_field(rest: &str, symbols: &std::collections::HashMap<String, usize>) -> u16 {
    let f = parse_lit(rest.split(',').next().unwrap().trim(), symbols);
    (f & 0xFF) as u16
}

fn parse_a_bit(rest: &str) -> u16 {
    match rest.to_ascii_uppercase().as_str() {
        "A" => 0,
        "B" => 1,
        other => panic!("asm(pic18): expected A or B, got {other}"),
    }
}

fn parse_d_bit(rest: &str) -> u16 {
    match rest.to_ascii_uppercase().as_str() {
        "W" => 0,
        "F" => 1,
        other => panic!("asm(pic18): expected W or F, got {other}"),
    }
}

/// Encode one PIC18 instruction line to 1 or 2 words. `addr` is this
/// instruction's own BYTE address (matching `symbols`, which `assemble_pic18`
/// also stores as byte addresses); the relative-branch/`GOTO`/`CALL` arms
/// divide by 2 to get the *word* address/offset the ISA's `k`/`n` fields need.
fn encode_pic18(addr: usize, line: &str, symbols: &std::collections::HashMap<String, usize>) -> Vec<u16> {
    let mut it = line.splitn(2, char::is_whitespace);
    let mne = it.next().expect("asm: empty instruction line").to_ascii_uppercase();
    let rest = it.next().unwrap_or("").trim();
    let ops: Vec<&str> = rest.split(',').map(str::trim).collect();
    match mne.as_str() {
        "NOP" => vec![0x0000],
        "ADDWF" | "ADDWFC" | "ANDWF" | "COMF" | "DECF" | "DECFSZ" | "DCFSNZ" | "INCF"
        | "INCFSZ" | "INFSNZ" | "IORWF" | "MOVF" | "RLCF" | "RLNCF" | "RRCF" | "RRNCF"
        | "SUBFWB" | "SUBWF" | "SUBWFB" | "SWAPF" | "XORWF" => {
            let f = parse_f_field(rest, symbols);
            let d = parse_d_bit(ops[1]);
            let a = parse_a_bit(ops[2]);
            let base: u16 = match mne.as_str() {
                "ADDWF" => 0x2400,
                "ADDWFC" => 0x2000,
                "ANDWF" => 0x1400,
                "COMF" => 0x1C00,
                "DECF" => 0x0400,
                "DECFSZ" => 0x2C00,
                "DCFSNZ" => 0x4C00,
                "INCF" => 0x2800,
                "INCFSZ" => 0x3C00,
                "INFSNZ" => 0x4800,
                "IORWF" => 0x1000,
                "MOVF" => 0x5000,
                "RLCF" => 0x3400,
                "RLNCF" => 0x4400,
                "RRCF" => 0x3000,
                "RRNCF" => 0x4000,
                "SUBFWB" => 0x5400,
                "SUBWF" => 0x5C00,
                "SUBWFB" => 0x5800,
                "SWAPF" => 0x3800,
                "XORWF" => 0x1800,
                _ => unreachable!(),
            };
            vec![base | d << 9 | a << 8 | f]
        }
        "CLRF" | "CPFSEQ" | "CPFSGT" | "CPFSLT" | "MOVWF" | "MULWF" | "NEGF" | "SETF"
        | "TSTFSZ" => {
            let f = parse_f_field(rest, symbols);
            let a = parse_a_bit(ops[1]);
            let base: u16 = match mne.as_str() {
                "CLRF" => 0x6A00,
                "CPFSEQ" => 0x6200,
                "CPFSGT" => 0x6400,
                "CPFSLT" => 0x6000,
                "MOVWF" => 0x6E00,
                "MULWF" => 0x0200,
                "NEGF" => 0x6C00,
                "SETF" => 0x6800,
                "TSTFSZ" => 0x6600,
                _ => unreachable!(),
            };
            vec![base | a << 8 | f]
        }
        "BCF" | "BSF" | "BTFSC" | "BTFSS" | "BTG" => {
            let f = parse_f_field(rest, symbols);
            let b: u16 = ops[1]
                .parse()
                .unwrap_or_else(|_| panic!("asm(pic18): bad bit number {}", ops[1]));
            assert!(b <= 7, "asm(pic18): bit number {b} out of range 0-7");
            let a = parse_a_bit(ops[2]);
            let base: u16 = match mne.as_str() {
                "BCF" => 0x9000,
                "BSF" => 0x8000,
                "BTFSC" => 0xB000,
                "BTFSS" => 0xA000,
                "BTG" => 0x7000,
                _ => unreachable!(),
            };
            vec![base | b << 9 | a << 8 | f]
        }
        "SUBLW" | "IORLW" | "XORLW" | "ANDLW" | "RETLW" | "MULLW" | "MOVLW" | "ADDLW" => {
            let k = (parse_lit(rest, symbols) & 0xFF) as u16;
            let base: u16 = match mne.as_str() {
                "SUBLW" => 0x0800,
                "IORLW" => 0x0900,
                "XORLW" => 0x0A00,
                "ANDLW" => 0x0B00,
                "RETLW" => 0x0C00,
                "MULLW" => 0x0D00,
                "MOVLW" => 0x0E00,
                "ADDLW" => 0x0F00,
                _ => unreachable!(),
            };
            vec![base | k]
        }
        "CLRWDT" => vec![0x0004],
        "TBLRD*" => vec![0x0008],
        "TBLRD*+" => vec![0x0009],
        "TBLRD*-" => vec![0x000A],
        "TBLRD+*" => vec![0x000B],
        "PUSH" => vec![0x0005],
        "POP" => vec![0x0006],
        "DAW" => vec![0x0007],
        "SLEEP" => vec![0x0003],
        "RESET" => vec![0x00FF],
        "RETFIE" | "RETURN" => {
            let s: u16 = if rest.eq_ignore_ascii_case("FAST") { 1 } else { 0 };
            let base: u16 = if mne == "RETFIE" { 0x0010 } else { 0x0012 };
            vec![base | s]
        }
        "MOVLB" => {
            let k = (parse_lit(rest, symbols) & 0xF) as u16;
            vec![0x0100 | k]
        }
        "BZ" | "BNZ" | "BC" | "BNC" | "BOV" | "BNOV" | "BN" | "BNN" => {
            let target = *symbols
                .get(rest)
                .unwrap_or_else(|| panic!("asm(pic18): undefined label {rest}"));
            // Branch offsets are word offsets; convert both byte addresses
            // to word addresses before taking the difference.
            let n = (target >> 1) as i32 - ((addr >> 1) as i32 + 1);
            assert!(
                (-128..=127).contains(&n),
                "asm(pic18): {mne} offset {n} out of range [-128,127]"
            );
            let n8 = (n as i8 as u8) as u16;
            let base: u16 = match mne.as_str() {
                "BZ" => 0xE000,
                "BNZ" => 0xE100,
                "BC" => 0xE200,
                "BNC" => 0xE300,
                "BOV" => 0xE400,
                "BNOV" => 0xE500,
                "BN" => 0xE600,
                "BNN" => 0xE700,
                _ => unreachable!(),
            };
            vec![base | n8]
        }
        "BRA" | "RCALL" => {
            let target = *symbols
                .get(rest)
                .unwrap_or_else(|| panic!("asm(pic18): undefined label {rest}"));
            let n = (target >> 1) as i32 - ((addr >> 1) as i32 + 1);
            assert!(
                (-1024..=1023).contains(&n),
                "asm(pic18): {mne} offset {n} out of range [-1024,1023]"
            );
            let n11 = (n as i16 as u16) & 0x7FF;
            let base: u16 = if mne == "BRA" { 0xD000 } else { 0xD800 };
            vec![base | n11]
        }
        "GOTO" => {
            let target = *symbols
                .get(rest)
                .unwrap_or_else(|| panic!("asm(pic18): undefined label {rest}"));
            let k = (target >> 1) as u32; // byte address -> word address
            vec![0xEF00 | (k & 0xFF) as u16, 0xF000 | ((k >> 8) & 0xFFF) as u16]
        }
        "CALL" => {
            let (label, fast) = match ops.as_slice() {
                [l] => (*l, false),
                [l, f] if f.eq_ignore_ascii_case("FAST") => (*l, true),
                _ => panic!("asm(pic18): CALL takes <label> or <label>,FAST"),
            };
            let target = *symbols
                .get(label)
                .unwrap_or_else(|| panic!("asm(pic18): undefined label {label}"));
            let k = (target >> 1) as u32; // byte address -> word address
            let s: u16 = if fast { 1 } else { 0 };
            vec![0xEC00 | s << 8 | (k & 0xFF) as u16, 0xF000 | ((k >> 8) & 0xFFF) as u16]
        }
        "LFSR" => {
            let fsr: u16 = ops[0]
                .parse()
                .unwrap_or_else(|_| panic!("asm(pic18): bad FSR number {}", ops[0]));
            assert!(fsr <= 2, "asm(pic18): FSR number {fsr} out of range 0-2");
            let k = (parse_lit(ops[1], symbols) & 0xFFF) as u16;
            vec![0xEE00 | fsr << 4 | (k >> 8), 0xF000 | (k & 0xFF)]
        }
        "MOVFF" => {
            let src_addr = (parse_lit(ops[0], symbols) & 0xFFF) as u16;
            let dst_addr = (parse_lit(ops[1], symbols) & 0xFFF) as u16;
            vec![0xC000 | src_addr, 0xF000 | dst_addr]
        }
        other => panic!("asm(pic18): unsupported mnemonic {other} (operand: {rest})"),
    }
}

/// Assemble source and render the result as Intel HEX.
///
/// The whole program (code + tables) must fit the device's flash: a program
/// whose highest word address is beyond `device.flash_words` panics loudly.
/// `assemble`/`assemble_pic18` are layout-only and stay unasserted so
/// isel's unit tests can inspect words of any size.
pub fn assemble_file_to_hex(device: &Device, src: &str) -> String {
    let words = match device.core {
        device::Core::Pic14 => assemble(src),
        device::Core::Pic18 => assemble_pic18(src),
    };
    assert!(
        words.len() as u32 <= device.flash_words,
        "asm: program of {} words exceeds device flash (highest address 0x{:04X} >= {:#06x}; {}-word flash)",
        words.len(),
        words.len().saturating_sub(1),
        device.flash_words,
        device.flash_words,
    );
    to_hex(&words)
}

fn parse_num(s: &str) -> usize {
    if let Some(h) = s.strip_prefix("0x") {
        usize::from_str_radix(h, 16).unwrap()
    } else {
        s.parse().unwrap()
    }
}

/// Case-insensitive strip of a `LOW(`/`HIGH(` prefix, dropping the trailing
/// `)`; returns the label name inside the parens.
fn strip_fn<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let (a, b) = (s.as_bytes(), name.as_bytes());
    if a.len() > b.len() && a[..b.len()].eq_ignore_ascii_case(b) {
        Some(s[b.len()..].trim_end_matches(')'))
    } else {
        None
    }
}

/// Resolve a literal operand. Plain numbers parse as-is; `LOW(<label>)` and
/// `HIGH(<label>)` resolve through the pass-2 symbol table to the low or high
/// byte of the label's word address (a RETLW table's base, e.g. the
/// `ADDLW LOW(table); MOVWF PCL` computed jump). `PAGE(<label>)` resolves to
/// `(addr >> 11) << 3` — the PCLATH<4:3> page bits (bits 2:0 clear), the
/// literal loaded into PCLATH before a cross-page CALL. `UPPER(<label>)`
/// resolves to byte 2 of the address (`(addr >> 16) & 0xFF`), the `TBLPTRU`
/// byte of a const table's base (zero for flash below 64 KiB, but the
/// encoding must still be emitted). A numeric operand inside the parens:
/// `LOW(0x2A)`, `HIGH(0x123)`, `LOW(35)`, `UPPER(0x12345)`, padded or
/// unpadded hex — resolves as the plain literal itself (LOW = n & 0xFF,
/// HIGH = (n >> 8) & 0xFF, PAGE = (n >> 11) << 3, UPPER = (n >> 16) & 0xFF),
/// the same semantics as the label form; gpasm accepts `LOW(<n>)`/
/// `HIGH(<n>)`, so a numeric operand is valid assembler input and must not
/// be treated as a missing label.
fn parse_lit(s: &str, sym: &std::collections::HashMap<String, usize>) -> usize {
    for (name, mask, shift) in [
        ("LOW(", 0xFFusize, 0usize),
        ("HIGH(", 0xFF, 8),
        ("PAGE(", 0x38, 8),
        ("UPPER(", 0xFF, 16),
    ] {
        if let Some(inner) = strip_fn(s, name) {
            // A label operand resolves through the symbol table; a number
            // operand (`0x` hex or decimal) evaluates on the value itself. A
            // name that is neither (a typo'd label) keeps the loud "label not
            // found" panic.
            if let Some(&v) = sym.get(inner) {
                return (v >> shift) & mask;
            }
            if let Some(n) = parse_num_opt(inner) {
                return (n >> shift) & mask;
            }
            panic!("asm: {name}{inner}) label not found");
        }
    }
    parse_num(s)
}

/// Parse a decimal or `0x`-prefixed hex number; `None` if it is neither.
fn parse_num_opt(s: &str) -> Option<usize> {
    if let Some(h) = s.strip_prefix("0x") {
        usize::from_str_radix(h, 16).ok()
    } else {
        s.parse().ok()
    }
}

fn encode(line: &str, sym: &std::collections::HashMap<String, usize>) -> u16 {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let mne = parts[0].to_ascii_uppercase();
    let op = parts.get(1).copied().unwrap_or("");
    let f = |s: &str| -> u16 {
        let t = s.trim_end_matches(',');
        let v = match sym.get(t) {
            Some(&v) => v,
            None => parse_num(t),
        };
        assert!(v <= 0x7F, "asm: file register 0x{v:02X} out of range");
        v as u16 & 0x7F
    };
    // Destination bit for the two-operand file ops (`f, W` / `f, F`): W = 0,
    // F = 1. An absent destination defaults to W, matching the encoding this
    // assembler always produced before destinations were parsed.
    let d = match parts.get(2).map(|s| s.trim().to_ascii_uppercase()).as_deref() {
        Some("F") => 1,
        _ => 0,
    };
    match mne.as_str() {
        "NOP" => 0x0000,
        "RETURN" => 0x0008,
        "RETFIE" => 0x0009,
        "SLEEP" => 0x0063,
        "CLRWDT" => 0x0064,
        "MOVWF" => 0x0080 | f(op),
        "CLRF" => 0x0180 | f(op),
        "MOVF" => 0x0800 | (d << 7) | f(op),
        "ADDWF" => 0x0700 | (d << 7) | f(op),
        "SUBWF" => 0x0200 | (d << 7) | f(op),
        "ANDWF" => 0x0500 | (d << 7) | f(op),
        "IORWF" => 0x0400 | (d << 7) | f(op),
        "XORWF" => 0x0600 | (d << 7) | f(op),
        "COMF" => 0x0900 | (d << 7) | f(op),
        "INCF" => 0x0A00 | (d << 7) | f(op),
        "DECFSZ" => 0x0B00 | (d << 7) | f(op),
        "RRF" => 0x0C00 | (d << 7) | f(op),
        "RLF" => 0x0D00 | (d << 7) | f(op),
        "SWAPF" => 0x0E00 | (d << 7) | f(op),
        "INCFSZ" => 0x0F00 | (d << 7) | f(op),
        "MOVLW" => 0x3000 | parse_lit(op, sym) as u16,
        "ADDLW" => 0x3E00 | parse_lit(op, sym) as u16,
        "ANDLW" => 0x3900 | parse_lit(op, sym) as u16,
        "IORLW" => 0x3800 | parse_lit(op, sym) as u16,
        "XORLW" => 0x3A00 | parse_lit(op, sym) as u16,
        "SUBLW" => 0x3C00 | parse_lit(op, sym) as u16,
        "RETLW" => 0x3400 | parse_lit(op, sym) as u16,
        "BTFSC" | "BTFSS" | "BCF" | "BSF" => {
            // Operands may be split across whitespace ("STATUS, 2"): join the
            // remaining tokens back into one operand string before splitting.
            let full = parts[1..].join(" ");
            let (freg, b) = full.split_once(',').unwrap();
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

/// Multi-region Intel HEX: each `(base_byte_addr, words)` chunk is written
/// in order, with a new `:04` extended-linear-address record emitted only
/// when a chunk's upper 16 address bits differ from the previous one. A
/// single chunk at base 0 produces output byte-identical to `to_hex`.
pub fn to_hex_regions(chunks: &[(u32, &[u16])]) -> String {
    let mut hex = String::new();
    let mut current_upper: Option<u32> = None;
    for &(base_byte_addr, words) in chunks {
        let upper = base_byte_addr >> 16;
        if current_upper != Some(upper) {
            let rec = [0x02, 0x00, 0x00, 0x04, (upper >> 8) as u8, (upper & 0xFF) as u8];
            hex.push_str(&hex_record(&rec));
            current_upper = Some(upper);
        }
        // Trim trailing zero words per chunk, matching `to_hex`'s own tail
        // trim. Config chunks are all 0xFF-erased (word 0xFFFF), so they are
        // never trimmed; only a program image's zero tail is.
        let hi = words.iter().rposition(|&w| w != 0).map(|i| i + 1).unwrap_or(0);
        let mut addr = 0usize;
        while addr < hi {
            let n = (hi - addr).min(8);
            let mut body = vec![0u8; 2 * n];
            for (i, w) in words[addr..addr + n].iter().enumerate() {
                body[2 * i] = (w & 0xFF) as u8;
                body[2 * i + 1] = ((w >> 8) & 0xFF) as u8;
            }
            let byte_addr = (base_byte_addr as usize & 0xFFFF) + addr * 2;
            let mut rec = vec![(2 * n) as u8, (byte_addr >> 8) as u8, (byte_addr & 0xFF) as u8, 0x00];
            rec.extend_from_slice(&body);
            hex.push_str(&hex_record(&rec));
            addr += n;
        }
    }
    hex.push_str(":00000001FF\n");
    hex
}

/// Render one Intel HEX record (byte count/address/type already in `rec`,
/// data appended) with its checksum, `:`-prefixed, newline-terminated.
fn hex_record(rec: &[u8]) -> String {
    let sum: u16 = rec.iter().map(|&b| b as u16).sum();
    let checksum = (0x100 - (sum & 0xFF)) as u8;
    let mut s = String::from(":");
    for b in rec {
        s.push_str(&format!("{b:02X}"));
    }
    s.push_str(&format!("{checksum:02X}\n"));
    s
}
