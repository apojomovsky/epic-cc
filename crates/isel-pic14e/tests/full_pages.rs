//! Cross-page packing pressure (epic-cc#1048): a bank-heavy function
//! sized past two 2048-word pages must keep every chunk inside one page
//! and run in sim. Planned chunk sizes over-approximate the final text
//! on this core (schedule, elision, and peephole only shrink past the
//! measure, and banking resets at every label), so the page-fit check
//! below holds by construction; this test locks the fit and the sim
//! result on a program that must occupy three pages.

use device::PIC16F1937;
use ir::parse;
use std::collections::{HashMap, HashSet};
fn globals_ir() -> String {
    let mut ir = String::new();
    for i in 0..7 {
        for j in 0..8 {
            ir.push_str(&format!("global g{i}_{j} i8\n"));
        }
    }
    ir.push_str("global in i8\nglobal out i8\n");
    ir
}

/// Seven chained blocks. Each block rotates cross-bank load/add/store
/// traffic, then extends the dead `acc` chain (`+0`) by `pads` links to
/// size the function past two pages. `out` always equals `in`.
fn build(pads: usize, traffic: usize) -> ir::Module {
    let mut ir = globals_ir();
    ir.push_str("fn main(void) ()\n  block entry:\n    %acc0 = load i8 @in\n");
    let mut reg = 0u32;
    let mut acc = "%acc0".to_string();
    for blk in 0..7 {
        if blk > 0 {
            ir.push_str(&format!("  block b{blk}:\n"));
        }
        for t in 0..traffic {
            let g = format!("g{}_{}", (blk + t) % 7, (blk * 3 + t) % 8);
            reg += 1;
            let r = format!("%t{reg}");
            ir.push_str(&format!("    {r} = load i8 @{g}\n"));
            reg += 1;
            let r2 = format!("%t{reg}");
            ir.push_str(&format!("    {r2} = add i8 {r}, 1\n"));
            ir.push_str(&format!("    store i8 {r2} @{g}\n"));
        }
        for _ in 0..pads {
            reg += 1;
            let r = format!("%t{reg}");
            ir.push_str(&format!("    {r} = add i8 {acc}, 0\n"));
            acc = r;
        }
        if blk < 6 {
            ir.push_str(&format!("    br b{}\n", blk + 1));
        }
    }
    ir.push_str(&format!("    store i8 {acc} @out\n    ret void\n"));
    parse(&ir)
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
fn full_pages_fit_and_run_in_sim() {
    // Sized past two full pages, so three chunks must occupy three pages.
    // `alloc` places every slot for real (folds drop theirs first), so the
    // sim result proves the shipped layout, not just the plan.
    let m = build(200, 8);
    let layout = alloc::allocate(&PIC16F1937, &m, "depth 1\n");
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let in_addr = addrs["in"];
    let out_addr = addrs["out"];
    let (asm, _, chunks, _) =
        isel_pic14e::select_with_locs(&PIC16F1937, &m, &addrs, &HashSet::new());
    let entries = chunks.get("main").expect("main must split");
    assert!(
        entries.len() >= 3,
        "past two pages needs at least three chunks:\n{asm}"
    );
    let pages: HashSet<usize> = entries
        .iter()
        .map(|e| label_addr(&asm, e) / 0x800)
        .collect();
    assert!(
        pages.len() >= 3,
        "chunks must occupy at least three pages:\n{asm}"
    );
    let scheduled = schedule::schedule(&PIC16F1937, &asm);
    let banked = banking::assign_banks(&PIC16F1937, &scheduled);
    let peeped = peephole::optimize(&banked);
    // Each chunk fits its page in the final layout: banking grew nothing
    // past the plan, or this panics naming the spilled chunk.
    isel_pic14e::verify_page_fit_split(&m, &peeped, &chunks);
    for input in [0u8, 5, 255] {
        assert_eq!(
            sim_run_asm(&peeped, &[(in_addr, input)], out_addr),
            input,
            "out must equal in={input} across full pages:\n{peeped}"
        );
    }
}
