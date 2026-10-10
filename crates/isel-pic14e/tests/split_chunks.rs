//! epic-cc#880 (mirror of epic-cc#841): a PIC14E function past one 2048-word
//! page is placed across pages in chunks linked by PCLATH-setting GOTOs.

use device::PIC16F1937;
use ir::parse;
use std::collections::{HashMap, HashSet};

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn label_addr(asm: &str, label: &str) -> usize {
    let mut org = 0usize;
    for raw in asm.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
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
            if l.trim() == label {
                return org;
            }
            continue;
        }
        if line.contains(" equ ") || line.starts_with(".table ") {
            continue;
        }
        if let Some(n) = line.strip_prefix(".align ") {
            let n: usize = n.trim().parse().unwrap();
            org = (org + n - 1) & !(n - 1);
            continue;
        }
        org += 1;
    }
    panic!("label {label} not found");
}

fn sim_run_asm(asm: &str, seed: &[(u16, u8)], out: u16) -> u8 {
    let words = asm::assemble_pic14e(asm);
    let mut p = pic14_sim::Pic14e::with_device(&PIC16F1937, words);
    for (a, v) in seed {
        p.ram_mut()[*a as usize] = *v;
    }
    p.run(2_000_000);
    assert!(p.halted(), "program must SLEEP-halt:\n{asm}");
    p.ram()[out as usize]
}

#[test]
fn split_function_spans_pages_and_runs_in_sim() {
    // main is well past 2048 words over six blocks (pads plus a counted
    // loop with a helper CALL), so it splits. The latch branches back, the
    // body calls across pages, and every intra-function GOTO carries its
    // target's page. Hand-traced outputs: in=0 -> 29, in=1 -> 16, in=2 -> 17.
    let mut entry_pad = String::new();
    for _ in 0..300 {
        entry_pad.push_str("    %d1 = add i8 %d1, 1\n");
    }
    let (mut pad2, mut pad3, mut pad4) = (String::new(), String::new(), String::new());
    for _ in 0..250 {
        pad2.push_str("    %d2 = add i8 %d2, 1\n");
        pad3.push_str("    %d3 = add i8 %d3, 1\n");
        pad4.push_str("    %d4 = add i8 %d4, 1\n");
    }
    let m = parse(&format!(
        "global in i8\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %a = load i8 @in\n    %n = add i8 %a, 0\n\
         \x20   %d1 = add i8 %a, 0\n{entry_pad}    br h1\n\
         block h1:\n    %i = phi i8 %n entry %m latch\n    %q = phi i8 %n entry %u latch\n\
         \x20   %i2 = add i8 %i, 1\n    %q2 = add i8 %q, 7\n    %d2 = add i8 %i2, 0\n{pad2}    br h2\n\
         block h2:\n    %j = phi i8 %i2 h1\n    %r = phi i8 %q2 h1\n    %t = call i8 @helper(i8 %r)\n\
         \x20   %r2 = add i8 %t, 5\n    %d3 = add i8 %r2, 0\n{pad3}    br h3\n\
         block h3:\n    %k = phi i8 %j h2\n    %s = phi i8 %r2 h2\n    %s2 = add i8 %s, 1\n\
         \x20   %d4 = add i8 %s2, 0\n{pad4}    br latch\n\
         block latch:\n    %m = phi i8 %k h3\n    %u = phi i8 %s2 h3\n    %c = icmp ult i8 %m, 2\n\
         \x20   br i1 %c h1 exit_h\n\
         block exit_h:\n    %v = add i8 %u, 1\n    store i8 %v @out\n    ret void\n\
         fn helper(i8) (x)\n  block entry:\n    %r = add i8 %x, 1\n    ret i8 %r\n"
    ));
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::a", 0x25),
        ("main::n", 0x26),
        ("main::d1", 0x27),
        ("main::i", 0x28),
        ("main::q", 0x29),
        ("main::i2", 0x2A),
        ("main::q2", 0x2B),
        ("main::d2", 0x2C),
        ("main::j", 0x2D),
        ("main::r", 0x2E),
        ("main::t", 0x2F),
        ("main::r2", 0x30),
        ("main::d3", 0x31),
        ("main::k", 0x32),
        ("main::s", 0x33),
        ("main::s2", 0x34),
        ("main::d4", 0x35),
        ("main::m", 0x36),
        ("main::u", 0x37),
        ("main::c", 0x38),
        ("main::v", 0x39),
        ("helper::x", 0x40),
        ("helper::r", 0x41),
    ]);
    let (asm, _, chunks) = isel_pic14e::select_with_locs(&PIC16F1937, &m, &addrs, &HashSet::new());
    let entries = chunks.get("main").expect("main must split");
    assert!(
        entries.len() >= 2,
        "main must place in at least two chunks:\n{asm}"
    );
    let pages: HashSet<usize> = entries
        .iter()
        .map(|e| label_addr(&asm, e) / 0x800)
        .collect();
    assert!(
        pages.len() >= 2,
        "chunks must land on distinct pages:\n{asm}"
    );
    // Every non-final chunk ends with a PCLATH-setting link to the next
    // chunk's entry label.
    for next in entries.iter().skip(1) {
        assert!(
            asm.contains(&format!(
                "MOVLW PAGE({next})\n    MOVWF PCLATH\n    GOTO {next}"
            )),
            "link to {next} must set PCLATH first:\n{asm}"
        );
    }
    let scheduled = schedule::schedule(&PIC16F1937, &asm);
    let banked = banking::assign_banks(&PIC16F1937, &scheduled);
    let peeped = peephole::optimize(&banked);
    // Each chunk fits its page in the final layout.
    isel_pic14e::verify_page_fit_split(&m, &peeped, &chunks);
    // The split program really runs across pages and back: the loop latch
    // returns to the header and the helper CALL restores PCLATH.
    assert_eq!(
        sim_run_asm(&peeped, &[(0x20, 0)], 0x21),
        29,
        "in=0 loops twice:\n{peeped}"
    );
    assert_eq!(
        sim_run_asm(&peeped, &[(0x20, 1)], 0x21),
        16,
        "in=1 exits after one iteration:\n{peeped}"
    );
    assert_eq!(
        sim_run_asm(&peeped, &[(0x20, 2)], 0x21),
        17,
        "in=2 exits after one iteration:\n{peeped}"
    );
}

/// Reads a `table_size`-byte const table through a 16-bit index in a function
/// that splits across pages. Returns (sim output, expected table byte).
fn split_const_read(table_size: usize, idx: u16) -> (u8, u8) {
    let mut pads = String::new();
    for _ in 0..400 {
        pads.push_str("    %a = add i8 %a, 1\n");
    }
    let mut m = parse(&format!(
        "global in i16\nglobal out i8\nconst t i8\n\
         fn main(void) ()\n  block entry:\n    %i = load i16 @in\n\
         \x20   %p = gep @t +0 +1*%i\n    %v = load i8 %p\n    store i8 %v @out\n\
         \x20   %a = add i8 %v, 0\n{pads}    br b1\n\
         block b1:\n{pads}    br b2\n\
         block b2:\n{pads}    br b3\n\
         block b3:\n{pads}    ret void\n"
    ));
    let bytes: Vec<u8> = (0..table_size)
        .map(|i| (i as u8) ^ ((i >> 8) as u8).wrapping_mul(0x5A).wrapping_add(1))
        .collect();
    let want = bytes[idx as usize];
    m.globals = vec![
        ir::Global {
            name: "in".into(),
            ty: ir::Ty::I16,
            is_const: false,
            size: 2,
            bytes: vec![0, 0],
            refs: Vec::new(),
            addr: None,
        },
        ir::Global {
            name: "out".into(),
            ty: ir::Ty::I8,
            is_const: false,
            size: 1,
            bytes: vec![0],
            refs: Vec::new(),
            addr: None,
        },
        ir::Global {
            name: "t".into(),
            ty: ir::Ty::I8,
            is_const: true,
            size: table_size as u16,
            bytes,
            refs: Vec::new(),
            addr: None,
        },
    ];
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x22),
        ("main::i", 0x24),
        ("main::v", 0x26),
        ("main::a", 0x27),
    ]);
    let (asm, _, chunks) = isel_pic14e::select_with_locs(&PIC16F1937, &m, &addrs, &HashSet::new());
    assert!(
        chunks.get("main").is_some_and(|c| c.len() >= 2),
        "main must split"
    );
    let seed = [(0x20, idx as u8), (0x21, (idx >> 8) as u8)];
    (sim_run_asm(&asm, &seed, 0x22), want)
}

#[test]
fn split_const_read_two_chunk_table_returns_table_byte() {
    // A 512-byte table is two chunks, so the read takes the bit-0 dispatch
    // whose join must still hold the loaded byte after the PCLATH restore.
    for idx in [0u16, 1, 255, 256, 300, 511] {
        let (got, want) = split_const_read(512, idx);
        assert_eq!(
            got, want,
            "t[{idx}] of a 512-byte table in a split function"
        );
    }
}

#[test]
fn split_const_read_chained_table_returns_table_byte() {
    // A 768-byte table is three chunks, so the read takes the >= chain.
    for idx in [0u16, 256, 511, 512, 600, 767] {
        let (got, want) = split_const_read(768, idx);
        assert_eq!(
            got, want,
            "t[{idx}] of a 768-byte table in a split function"
        );
    }
}
