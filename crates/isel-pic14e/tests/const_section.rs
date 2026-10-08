//! epic-cc#933: staged consts' `__stage_` routines must count in the
//! const section model, otherwise a later table's window fold runs at an
//! address hundreds of words early and the real base crosses its window.

use device::PIC16F1937;
use ir::parse;
use std::collections::{HashMap, HashSet};

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn const_table_global(name: &str, size: usize) -> ir::Global {
    let bytes: Vec<u8> = (0..size)
        .map(|i| {
            if i < 256 {
                i as u8
            } else {
                0x11 + (i - 256) as u8
            }
        })
        .collect();
    ir::Global {
        name: name.into(),
        ty: ir::Ty::I8,
        is_const: true,
        size: size as u16,
        bytes,
        refs: Vec::new(),
        addr: None,
    }
}

fn module_with_globals(ir_text: &str, globals: Vec<ir::Global>) -> ir::Module {
    let mut m = parse(ir_text);
    m.globals = globals;
    m
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
    p.run(200_000);
    assert!(p.halted(), "program must SLEEP-halt:\n{asm}");
    p.ram()[out as usize]
}

fn pad_body(n: usize) -> String {
    let mut body = String::new();
    for _ in 0..n {
        body.push_str("    %a = add i8 %a, 1\n");
    }
    body
}

#[test]
fn staged_routine_words_fold_into_later_table_window() {
    // epic-cc#933 (mirror of epic-cc#844): staged const `a` (10 bytes,
    // 51-word nominal routine) must shift table `t` (48 bytes) in the
    // section model. main is padded so the fold fires only when the
    // routine counts: without it the base sits inside the window and no
    // pad emits, and the assembler would panic on the crossing base.
    let ir_text = format!(
        "global in i8\nglobal out i8\nconst t i8\nconst a i8\nfn main(void) ()\n  block entry:\n           %i = load i8 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n    store i8 %v @out\n{}    ret void\n",
        pad_body(132)
    );
    let m = module_with_globals(
        &ir_text,
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("t", 48),
            const_table_global("a", 10),
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::i", 0x25),
        ("main::v", 0x26),
        ("main::a", 0x27),
        ("__const_stage", 0x30),
    ]);
    let mut staged = HashSet::new();
    staged.insert("a".to_string());
    let asm = isel_pic14e::select_with_locs(&PIC16F1937, &m, &addrs, &staged).0;
    let base = label_addr(&asm, "t");
    assert!(
        base & 0xFF == 0,
        "crossing base must be 256-aligned (base 0x{base:03X}):\n{asm}"
    );
    assert!(
        asm.contains("    .align 256\n    .table t 48"),
        "the .align 256 must precede the .table directive:\n{asm}"
    );
    isel_pic14e::verify_page_fit(&m, &asm);
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 5)], 0x21),
        5,
        "table[5] = 5 through the folded window:\n{asm}"
    );
}

#[test]
fn const_reader_at_page_tail_pins_to_next_page() {
    // epic-cc#933: a 6-word reader placed at a page tail straddled into the
    // next page. The emitter now pins a straddling reader with `.org` to
    // the next page. main is padded so the section starts with the reader
    // straddling 0x7FF/0x800; re-tune the pad when call sequences change.
    let ir_text = format!(
        "global in i8\nglobal out i8\nconst t i8\nfn main(void) ()\n  block entry:\n    %i = load i8 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n    store i8 %v @out\n{}    ret void\n",
        pad_body(675)
    );
    let m = module_with_globals(
        &ir_text,
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            const_table_global("t", 60),
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::i", 0x25),
        ("main::v", 0x26),
        ("main::a", 0x27),
    ]);
    let asm = isel_pic14e::select(&PIC16F1937, &m, &addrs);
    assert!(
        asm.contains("    org 0x0800\n__read_t:"),
        "the straddling reader must pin to page 1:\n{asm}"
    );
    assert_eq!(
        label_addr(&asm, "__read_t"),
        0x800,
        "pinned reader at the page start:\n{asm}"
    );
    let base = label_addr(&asm, "t");
    assert!(
        (base & 0xFF) + 60 <= 0x100,
        "pinned table must fit its window (base 0x{base:03X}):\n{asm}"
    );
    isel_pic14e::verify_page_fit(&m, &asm);
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 3)], 0x21),
        3,
        "table[3] = 3 through the pinned reader:\n{asm}"
    );
}

#[test]
fn three_chunk_table_followup_base_counts_all_chunks() {
    // A 600-byte table is three chunks; the old restore-page walk overcounted
    // the section by 256 words past the second chunk, so a later table's
    // restore map pointed one page too late. helper reads `t` whose real base
    // sits late in page 0 while the old model put it in page 1: the post-call
    // restore was kept although caller and base share page 0. main is padded
    // so the real base lands late-page-0 with margin either way; re-tune the
    // pad when call sequences change length.
    let ir_text = format!(
        "global in i8\nglobal out i8\nconst big i8\nconst t i8\nfn main(void) ()\n  block entry:\n{}    call void @helper()\n    ret void\nfn helper(void) ()\n  block entry:\n    %i = load i8 @in\n    %p = gep @t +0 +1*%i\n    %v = load i8 %p\n    %c = icmp eq i8 %v, 5\n    br i1 %c then end\n  block then:\n    store i8 1 @out\n    ret void\n  block end:\n    store i8 2 @out\n    ret void\n",
        pad_body(360)
    );
    let m = module_with_globals(
        &ir_text,
        vec![
            ir::Global {
                name: "in".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
            ir::Global {
                name: "big".into(),
                ty: ir::Ty::I8,
                is_const: true,
                size: 600,
                bytes: (0..600).map(|i| (i % 251) as u8).collect(),
                refs: Vec::new(),
                addr: None,
            },
            const_table_global("t", 48),
        ],
    );
    let addrs = addrs(&[
        ("in", 0x20),
        ("out", 0x21),
        ("main::a", 0x25),
        ("helper::i", 0x26),
        ("helper::v", 0x27),
        ("helper::c", 0x28),
    ]);
    let asm = isel_pic14e::select(&PIC16F1937, &m, &addrs);
    let base = label_addr(&asm, "t");
    assert!(
        base / 0x800 == 0 && (base & 0x7FF) >= 0x700,
        "follow-up base must sit late in page 0 (base 0x{base:03X}):\n{asm}"
    );
    assert!(
        (base & 0xFF) + 48 <= 0x100,
        "follow-up table must fit its window (base 0x{base:03X}):\n{asm}"
    );
    // Same page twice over (caller page 0, real base page 0): the restore
    // after the CALL is skipped, the byte travels CALL to scratch to slot.
    assert!(
        !asm.contains("CALL __read_t\n    MOVWF 0x70\n    MOVLW PAGE(helper)\n    MOVWF PCLATH"),
        "same-page reader restore must be skipped:\n{asm}"
    );
    isel_pic14e::verify_page_fit(&m, &asm);
    assert_eq!(
        sim_run_asm(&asm, &[(0x20, 5)], 0x21),
        1,
        "t[5] = 5 takes the then arm with the restore skipped:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "255-byte single-chunk staging bound")]
fn panics_on_staged_const_over_255_bytes() {
    // epic-cc#934: the `__stage_` routine indexes with one MOVLW byte
    // and always CALLs the chunk-0 reader, so a staged const past 255
    // bytes would misread. alloc never stages such consts, so the set
    // is hand-built here; emission must fail loudly, never copy wrong.
    let m = module_with_globals(
        "const big i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    store i8 0 @out\n    ret void\n",
        vec![
            const_table_global("big", 256),
            ir::Global {
                name: "out".into(),
                ty: ir::Ty::I8,
                is_const: false,
                size: 1,
                bytes: vec![0],
                addr: None,
                refs: Vec::new(),
            },
        ],
    );
    let addrs = addrs(&[("out", 0x20), ("__const_stage", 0x30)]);
    let mut staged = HashSet::new();
    staged.insert("big".to_string());
    let _ = isel_pic14e::select_with_locs(&PIC16F1937, &m, &addrs, &staged);
}
