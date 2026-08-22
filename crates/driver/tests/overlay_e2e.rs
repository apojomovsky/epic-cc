//! Milestone-3 overlay acceptance: two sibling functions (big_a, big_b) each
//! carry >= 16 bytes of simultaneous live i16 locals, called sequentially
//! from main. Acceptance: (a) the program runs correctly in the simulator,
//! (b) the local address map shows big_a and big_b sharing a base region
//! (overlay), and total_bank0 < locals_size(big_a) + locals_size(big_b) +
//! locals_size(main).

use std::collections::HashMap;
use std::process::Command;

use ir::{Inst, Module};

/// Width of every local (params + defined values, each name once, icmp -> i1)
/// of `fname` — the same rule alloc uses to size frames.
fn local_widths(m: &Module, fname: &str) -> HashMap<String, u8> {
    let f = m.funcs.iter().find(|f| f.name == fname).expect("function");
    let mut widths: HashMap<String, u8> = HashMap::new();
    for p in &f.params {
        widths.insert(p.name.clone(), p.width);
    }
    for b in &f.blocks {
        for inst in &b.insts {
            let (name, w) = match inst {
                Inst::Load(l) => (l.dst.clone(), l.ty.bytes()),
                Inst::Bin(b) => (b.dst.clone(), b.ty.bytes()),
                Inst::Zext(z) => (z.dst.clone(), z.to.bytes()),
                Inst::Trunc(t) => (t.dst.clone(), t.to.bytes()),
                Inst::Icmp(i) => (i.dst.clone(), 1),
                Inst::Select(s) => (s.dst.clone(), s.ty.bytes()),
                Inst::Call(c) => match (&c.dst, &c.ty) {
                    (Some(d), Some(t)) => (d.clone(), t.bytes()),
                    _ => continue,
                },
                Inst::Phi(p) => (p.dst.clone(), p.ty.bytes()),
                _ => continue,
            };
            widths.insert(name, w);
        }
    }
    widths
}

/// The map's span for a function: max(addr + width) - min(addr) over its
/// locals — the bytes of simultaneous locals its frame demands.
fn map_span(layout: &alloc::AllocLayout, fname: &str, widths: &HashMap<String, u8>) -> u16 {
    let prefix = format!("{fname}::");
    let mut min_addr: Option<u16> = None;
    let mut max_end: u16 = 0;
    for (key, &addr) in &layout.locals {
        if let Some(name) = key.strip_prefix(&prefix) {
            let w = u16::from(*widths.get(name).expect("local width"));
            min_addr = Some(min_addr.map_or(u16::from(addr), |m| m.min(u16::from(addr))));
            max_end = max_end.max(u16::from(addr) + w);
        }
    }
    max_end - min_addr.expect("function has map locals")
}

/// The lowest address of a function's frame (its base).
fn base_of(layout: &alloc::AllocLayout, fname: &str) -> u16 {
    let prefix = format!("{fname}::");
    layout
        .locals
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, &a)| u16::from(a))
        .min()
        .expect("function has map locals")
}

/// Run clang + the full IR pipeline on the overlay fixture, exactly as the
/// driver does, and return the alloc layout.
fn overlay_layout() -> (Module, alloc::AllocLayout) {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let ll = Command::new(clang)
        .args([
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-resource-dir",
            &resdir,
            "-o",
            "-",
            "tests/fixtures/overlay.c",
        ])
        .output()
        .expect("run clang");
    assert!(
        ll.status.success(),
        "clang: {}",
        String::from_utf8_lossy(&ll.stderr)
    );
    let ll_text = String::from_utf8(ll.stdout).unwrap();

    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    (m, layout)
}

#[test]
fn overlay_runs_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/overlay.c",
            "-o",
            "tests/fixtures/overlay.hex",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let hex = std::fs::read_to_string("tests/fixtures/overlay.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 3; // in = 3
    p.run(500_000);
    // big_a(3) = 3+4+..+10 = 52; big_b(in+1=4) = 0+1+2+3+5+6+7+8 = 32;
    // out = (unsigned char)(52 + 32) = 84
    assert_eq!(p.ram()[0x21], 84);
    assert!(p.halted());
}

#[test]
fn overlay_frames_share_ram() {
    let (m, layout) = overlay_layout();

    // The critical .ll property: each sibling carries >= 16 bytes of
    // simultaneous i16 locals (else -O1 folded the program away).
    let span_a = map_span(&layout, "big_a", &local_widths(&m, "big_a"));
    let span_b = map_span(&layout, "big_b", &local_widths(&m, "big_b"));
    let span_main = map_span(&layout, "main", &local_widths(&m, "main"));
    assert!(span_a >= 16 && span_b >= 16,
        "each sibling must carry >= 16 bytes of simultaneous locals (got big_a={span_a}, big_b={span_b})");

    // (b) sibling frames overlay: identical base region (never co-live).
    assert_eq!(
        base_of(&layout, "big_a"),
        base_of(&layout, "big_b"),
        "big_a and big_b must share a base address"
    );

    // main's frame is disjoint and sits before the shared sibling region.
    assert!(base_of(&layout, "main") + span_main <= base_of(&layout, "big_a"));

    // Overlay wins: total bank-0 demand < sum of the three demands.
    let sum_demands = span_a + span_b + span_main;
    assert!(
        layout.total_bank0 < sum_demands,
        "total_bank0 {} must be < sum of individual demands {}",
        layout.total_bank0,
        sum_demands
    );
}
