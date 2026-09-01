//! Milestone-11 multi-page acceptance: a > 2KB program with functions spread
//! across pages 0-2, a const table in page 2, cross-page calls (f2 -> f1,
//! f3 -> f2, main -> f1) and cross-page const-reader calls (main/f3 ->
//! `__read_table`/`__read_table_hi` in page 2), all simulated to a
//! hand-computed `out`.
//!
//! Fixture: `multi_page.c`. The functions in source order (clang -O1 emits
//! them in order, so module order = source order):
//!   f1, F1, F2, f2, F3, f3, main, F4
//! where f1/f2/f3 form the noinline chain (f3 calls f2 calls f1; f3 and main
//! also read the 300-byte `table`), and F1..F4 are noinline *uncalled*
//! arithmetic fillers whose sole job is to fill pages (their RAM frames
//! overlay — never live — so they cost no RAM; they are kept by clang
//! because they have external linkage). `main` is deliberately NOT the first
//! function: everything before it pushes it into page 2, so `__start`'s
//! `MOVLW PAGE(main)` loads a nonzero literal (0x10) — the Task-2 reviewer's
//! coverage gap (a main at 0x0005 would load PAGE(main) = 0x00 and never
//! exercise the PCLATH set).
//!
//! Main-padding/recipe-frame overlap: `main`'s six `(p + 3) * K / M` padding
//! steps (before the three real statements) each call the `__mul_u8`/
//! `__udiv_u8` recipes. The overlay allocator bases every callee at its
//! caller's physical frame end, so the recipe slots sit right after `main`'s
//! frame — at 0x2A, exactly where `f3`'s frame also starts (f3 is another
//! main callee). The two regions overlap but are NEVER simultaneously live:
//! main's padding (and its recipe calls) runs before the `f3` call, so the
//! recipe slots and f3's frame are used at disjoint times. This is safe by
//! construction (padding is pure dead code that must run before the real
//! statements), but it is load-bearing: moving the padding after the f3
//! call, or letting main call f3 before the padding, would corrupt f3's
//! frame — documented here per the task-4 review.
//!
//!
//! The layout below is the liveness-overlay one (epic-cc#172): the uncalled
//! fillers' frames collapse to their live peak (F2: 1261 -> 951 words of
//! body), so the first-fit bin packing re-packs the pages. The chain and
//! the readers still sit in the designed pages, main still lands in a
//! NONZERO page, and the table section still sits in page 2.
//!
//! ```text
//! __start        0x0001  page 0
//! f1             0x0005  page 0   (chain root)
//! F1             0x0170  page 0   (filler)
//! f2             0x0527  page 0   (calls f1   -> same page)
//! f3             0x0663  page 0   (calls f2   -> same page; reads table)
//! __mul_u8       0x06E6  page 0
//! __udiv_u8      0x0704  page 0
//! F2             0x0800  page 1   (filler, .org-padded)
//! F3             0x0BB7  page 1   (filler)
//! main           0x1000  page 2   (calls f3/f1 cross-page; reads table)
//! F4             0x1125  page 2   (filler)
//! __read_table   0x14DC  page 2   (cross-page from f3; same-page from main)
//! table          0x1500  page 2   (256-byte window 0x15 — the reader's
//!                                   `MOVLW HIGH(table); MOVWF PCLATH` is
//!                                   load-bearing: without it the computed
//!                                   PCL jump would land in window 0x10)
//! table_1        0x1600  page 2
//! __read_table_hi 0x162C page 2   (same-page from main)
//! ```
//!
//! Total program: 0x1632 = 5682 words (> 0x800, < 0x2000 device bound).
//!
//! Cross-page CALL sites in the final asm: f2 -> f1 (page 0 -> 0, same
//! page), f3 -> f2 (0 -> 0, same page), f3 -> __read_table (0 -> 2), main
//! -> f3 (2 -> 0), main -> f1 (2 -> 0), main -> __read_table (2 -> 2, same
//! page), main -> __read_table_hi (2 -> 2, same page). The cross-page calls
//! carry the `MOVLW PAGE(t); MOVWF PCLATH` set before and the `MOVLW
//! PAGE(cur); MOVWF PCLATH` restore after; the same-page calls skip the
//! restore (the caller's page is still in PCLATH). The same-page
//! restore-skip discipline is covered by the isel unit tests
//! (`same_page_call_skips_restore`, `multi_page_module_runs_in_sim`).
//!
//! `out` for in == 290 (0x0122), hand-computed against the exact emitted IR
//! (clang -O1 folds arithmetic, so the trace is the IR, not the C — the
//! evaluator in the task's generator reproduces it; the fixture's volatile
//! reads keep every table read runtime):
//!   - in = 290 -> f3's argument x = (unsigned char)in = 34 (0x22)
//!   - f1(34) = 0x5C, f2(34) = 0x53, f3(34) = 0x4B — each function is the
//!     add/xor chain (add an odd constant, xor the input, repeat; constants
//!     read from the exact emitted IR), with f2 adding f1(x) and f3 adding
//!     f2(x) plus table[x & 3] = table[2] = 0x02. Evaluated over u8
//!     wraparound from the IR op list.
//!   - out = f3((unsigned char)in) = 0x4B
//!   - out += f1(table[in & 3]) = f1(table[2]) = f1(0x02) = 0x7C -> 0xC7
//!   - out += table[in - 34] = table[256] = 0x11 -> 0xD8
//!   - out = 0xD8 = 216
//!
//! The simulation below is the ground truth; the intermediate values come
//! from evaluating the exact emitted IR with a Python evaluator (documented
//! in the task report). `in` is the i16 global at 0x20-0x21 (290 = 0x0122);
//! `out` is the u8 global at 0x22 (read from the alloc layout, not
//! hardcoded).

use std::collections::HashMap;
use std::process::Command;

/// Run the exact driver pipeline in-process and return (final asm, layout).
fn multi_page_pipeline() -> (String, alloc::AllocLayout) {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/multi_page.c"),
        &driver::clang::Options::default(),
    );

    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, 8);
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel::select(&device::PIC16F877A, &m, &addrs);
    let asm = banking::assign_banks(&device::PIC16F877A, &asm);
    let asm = peephole::optimize(&asm);
    (asm, layout)
}

/// Walk the final asm exactly as `asm::assemble` pass 1 does (org / .align /
/// labels / word lines) and return label -> word address.
fn label_addrs(asm: &str) -> HashMap<String, usize> {
    let mut org = 0usize;
    let mut out = HashMap::new();
    for raw in asm.lines() {
        let t = raw.split(';').next().unwrap_or("").trim();
        if t.is_empty() || t.starts_with("list") || t.starts_with("radix") || t.contains(" equ ") {
            continue;
        }
        if let Some(rest) = t.strip_prefix("org ") {
            org = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
        } else if let Some(n) = t.strip_prefix(".align ") {
            let n: usize = n.trim().parse().unwrap();
            org = (org + n - 1) & !(n - 1);
        } else if let Some(l) = t.strip_suffix(':') {
            out.insert(l.trim().to_string(), org);
        } else if t.starts_with(".table ") || t.starts_with("end") {
            // no words
        } else {
            org += 1;
        }
    }
    out
}

#[test]
fn multi_page_program_compiles_and_runs_correctly() {
    let (asm, layout) = multi_page_pipeline();
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    // The final assembled layout: every label's page == the page the isel
    // greedy assignment put it in (no post-banking straddle). Folded-in
    // coverage (b): check each function's final address against its
    // assigned page.
    let addrs = label_addrs(&asm);
    let page = |label: &str| addrs.get(label).map(|a| a >> 11);

    // The chain and the readers must sit in the designed pages (see the
    // layout table above) — in particular main must be in a NONZERO page
    // (so __start's PAGE(main) is a nonzero literal). The bin-packed
    // layout (issue #12) fills page tails with post-banking sizes: f2, f3
    // and the mul/div recipes land in page 0's tail after F1, main lands
    // in page 2, and the table section sits in page 2.
    assert_eq!(page("f1"), Some(0), "f1 in page 0");
    assert_eq!(page("F1"), Some(0), "F1 in page 0");
    assert_eq!(
        page("f2"),
        Some(0),
        "f2 in page 0 (bin-packed into the tail)"
    );
    assert_eq!(
        page("f3"),
        Some(0),
        "f3 in page 0 (bin-packed into the tail)"
    );
    assert_eq!(page("F2"), Some(1), "F2 in page 1");
    assert_eq!(page("F3"), Some(1), "F3 in page 1");
    assert_eq!(
        page("main"),
        Some(2),
        "main in page 2 (nonzero, PAGE(main) != 0)"
    );
    assert_eq!(page("F4"), Some(2), "F4 in page 2");
    assert_eq!(page("__read_table"), Some(2), "reader in page 2");
    assert_eq!(
        page("table"),
        Some(2),
        "table in page 2 (later than every function)"
    );
    assert_eq!(page("table_1"), Some(2), "table chunk 1 in page 2");
    assert_eq!(page("__read_table_hi"), Some(2), "chunk-1 reader in page 2");

    // The cross-page CALL sites must really cross (page of caller's label vs
    // page of the target's label, from the assembled layout). The chain
    // calls are same-page (f2 -> f1 is 0 -> 0, f3 -> f2 is 0 -> 0), main's
    // calls are cross-page (main -> f3 is 2 -> 0, main -> f1 is 2 -> 0),
    // and the readers are cross-page from f3 (0 -> 2) but same-page from
    // main (2 -> 2) — the same-page restore-skip discipline is covered by
    // the isel unit tests (`same_page_call_skips_restore`,
    // `multi_page_module_runs_in_sim`).
    assert_eq!(page("f2"), page("f1"), "f2 -> f1 is same-page");
    assert_eq!(page("f3"), page("f2"), "f3 -> f2 is same-page");
    assert_ne!(page("main"), page("f3"), "main -> f3 is cross-page");
    assert_ne!(page("main"), page("f1"), "main -> f1 is cross-page");
    assert_ne!(
        page("f3"),
        page("__read_table"),
        "f3 -> reader is cross-page"
    );
    assert_eq!(
        page("main"),
        page("__read_table"),
        "main -> reader is same-page"
    );
    assert_eq!(
        page("main"),
        page("__read_table_hi"),
        "main -> hi reader is same-page"
    );

    // The program must exceed 0x800 words (the whole point of multi-page).
    let total = *addrs.get("__read_table_hi").unwrap() + 6;
    assert!(
        total > 0x800,
        "program is {total} words (must exceed 0x800)"
    );
    assert!(
        total < 0x2000,
        "program fits the 8K-word device flash (0x{total:04X})"
    );

    // ---- run the driver and simulate ----
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/multi_page.c",
            "-o",
            "tests/fixtures/multi_page.hex",
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

    let hex = std::fs::read_to_string("tests/fixtures/multi_page.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 0x22; // in low byte = 290 & 0xFF
    p.ram_mut()[0x21] = 0x01; // in high byte = 290 >> 8
    p.run(2_000_000);
    assert_eq!(
        p.ram()[out_addr],
        0xD8,
        "out == hand-computed 0xD8 for in == 290 (trace in the module docs)"
    );
    assert!(p.halted(), "program halts (SLEEP after main returns)");
}
