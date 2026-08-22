//! Issue #2 acceptance: a program whose main AND ISR both reach the injected
//! `__mul_u8` runtime routine compiles through the whole driver pipeline,
//! the ISR context gets its OWN routine copy, and the two copies' frames do
//! not overlap.
//!
//! Frame disjointness is the property that makes the clobber impossible. A
//! single shared `__mul_u8` frame means an interrupt taken partway through
//! main's shift-add loop overwrites the multiplier, the counter and the
//! running product, and main resumes against the ISR's values — a silent
//! wrong answer, no diagnostic. See `fixtures/interrupt_mul.c`.
//!
//! This test does not fire an interrupt mid-routine: `Pic14::fire_interrupt`
//! pushes `pc + 1`, so the instruction at the injection point never runs and
//! any mid-routine injection would measure that simulator behaviour instead
//! of the fix. Tracked separately (issue #15).
use std::collections::HashMap;
use std::process::Command;

/// Run the pipeline stages the driver runs, returning the layout and the
/// emitted (pre-banking) assembly.
fn compile_fixture() -> (alloc::AllocLayout, String) {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/interrupt_mul.c"),
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
    (layout, asm)
}

/// `__mul_u8`'s frame: the two 1-byte params and the 6-byte scratch area
/// (`routine_func` in crates/legalize sizes the alloca).
fn frame_ranges(layout: &alloc::AllocLayout, func: &str) -> Vec<(u16, u16)> {
    let slot = |name: &str| {
        *layout
            .locals
            .get(&format!("{func}::{name}"))
            .unwrap_or_else(|| panic!("no slot {func}::{name} in the layout"))
    };
    vec![(slot("a"), 1), (slot("b"), 1), (slot("__scr"), 6)]
}

#[test]
fn isr_gets_its_own_multiply_routine_with_a_disjoint_frame() {
    let (layout, _asm) = compile_fixture();

    // Both copies exist: main keeps `__mul_u8`, the ISR context gets its own.
    assert!(
        layout.locals.contains_key("__mul_u8::__scr"),
        "main's routine frame is missing from the layout"
    );
    assert!(
        layout.locals.contains_key("__mul_u8_isr::__scr"),
        "the ISR context must get its own __mul_u8_isr copy; layout has no __mul_u8_isr::__scr"
    );

    // No byte of main's routine frame is also part of the ISR copy's frame.
    let main_frame = frame_ranges(&layout, "__mul_u8");
    let isr_frame = frame_ranges(&layout, "__mul_u8_isr");
    for (ma, msz) in &main_frame {
        for (ia, isz) in &isr_frame {
            let overlap = *ma < ia + isz && *ia < ma + msz;
            assert!(
                !overlap,
                "__mul_u8 frame [0x{ma:02X},+{msz}) overlaps __mul_u8_isr [0x{ia:02X},+{isz}) — \
                 an interrupt during main's multiply would clobber it"
            );
        }
    }
}

#[test]
fn both_routine_copies_emit_their_own_body() {
    let (_layout, asm) = compile_fixture();
    assert!(
        asm.contains("__mul_u8:"),
        "main's routine body is missing:\n{asm}"
    );
    assert!(
        asm.contains("__mul_u8_isr:"),
        "the ISR routine copy must emit its own body:\n{asm}"
    );
    // The ISR's call targets the copy, not the shared original.
    assert!(
        asm.contains("CALL __mul_u8_isr"),
        "the ISR must call its own copy:\n{asm}"
    );
}

/// A routine body: every line from `label:` through its terminating
/// `RETURN`, internal labels (`tmp7:`) included — they are part of the body
/// and a truncated comparison would miss the retval store at the end.
fn body_of(asm: &str, label: &str) -> Vec<String> {
    let mut lines = asm.lines().skip_while(|l| l.trim() != format!("{label}:"));
    lines
        .next()
        .unwrap_or_else(|| panic!("no @{label} in the emitted asm"));
    let mut body = Vec::new();
    for l in lines {
        let t = l.trim();
        if t.is_empty() {
            continue;
        }
        body.push(t.to_string());
        if t == "RETURN" {
            return body;
        }
    }
    panic!("@{label}'s body has no RETURN");
}

/// The `[base, span)` of a function's frame, from the allocated slots. The
/// margin past the highest slot covers the scratch bytes, which alloc keys
/// by the `__scr` base only (the widest routine scratch is 14 bytes).
fn frame_of(layout: &alloc::AllocLayout, f: &str) -> (u16, u16) {
    let prefix = format!("{f}::");
    let addrs: Vec<u16> = layout
        .locals
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, v)| *v)
        .collect();
    assert!(!addrs.is_empty(), "no slots for {f} in the layout");
    let base = *addrs.iter().min().unwrap();
    (base, addrs.iter().max().unwrap() - base + 16)
}

/// Rewrite a body into a frame-base- and label-independent form: operands
/// inside `[base, base + span)` become `F+<offset>`, and the generated
/// `tmpN` labels become `L0`, `L1`, … in order of first appearance. Two
/// copies of one routine then compare equal, while a genuinely different
/// operand or a different control-flow shape still differs.
fn normalize(body: &[String], base: u16, span: u16) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    for line in body {
        for tok in line.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
            if let Some(n) = tok.strip_prefix("tmp") {
                if !n.is_empty()
                    && n.chars().all(|c| c.is_ascii_digit())
                    && !labels.iter().any(|l| l == tok)
                {
                    labels.push(tok.to_string());
                }
            }
        }
    }
    // Longest name first, so replacing `tmp1` cannot corrupt `tmp12`.
    let mut order: Vec<(usize, &String)> = labels.iter().enumerate().collect();
    order.sort_by_key(|(_, s)| std::cmp::Reverse(s.len()));
    body.iter()
        .map(|line| {
            let mut out = line.clone();
            for a in base..base + span {
                out = out.replace(&format!("0x{a:02X}"), &format!("F+{}", a - base));
            }
            for (i, name) in &order {
                out = out.replace(name.as_str(), &format!("L{i}"));
            }
            out
        })
        .collect()
}

/// Every `_isr` routine copy must emit the SAME body as its original, once
/// the frame base is factored out.
///
/// `emit_routine` picks its recipe by function name, and several routines
/// discriminate on that name inside the recipe — `__udiv_u8` vs `__urem_u8`
/// share one loop and differ only in which byte they return. A copy that
/// matched on its own `__udiv_u8_isr` name would silently take the `else`
/// arm and return the remainder. This pins every such site at once, for
/// whatever routines the fixture happens to use.
#[test]
fn each_isr_routine_copy_matches_its_original_body() {
    let (layout, asm) = compile_fixture();
    let copies: Vec<String> = asm
        .lines()
        .filter_map(|l| l.trim().strip_suffix(':').map(str::to_string))
        .filter(|l| l.starts_with("__") && l.ends_with("_isr"))
        .collect();
    assert!(
        !copies.is_empty(),
        "the fixture must produce at least one _isr routine copy"
    );
    // The fixture is written to exercise the name-discriminating recipes,
    // so the divide copy must actually be among them.
    assert!(
        copies.iter().any(|c| c == "__udiv_u8_isr"),
        "the fixture must duplicate __udiv_u8 (the quotient/remainder recipe): {copies:?}"
    );

    for copy in &copies {
        let base_name = copy.strip_suffix("_isr").unwrap();
        let (ob, os) = frame_of(&layout, base_name);
        let (cb, cs) = frame_of(&layout, copy);
        let orig = normalize(&body_of(&asm, base_name), ob, os);
        let dup = normalize(&body_of(&asm, copy), cb, cs);
        assert_eq!(
            orig, dup,
            "@{copy} must emit the same body as @{base_name} modulo its frame base"
        );
    }
}

#[test]
fn the_program_still_computes_mains_results() {
    // A no-interrupt sanity run: the duplication must not disturb the
    // ordinary path. in_a = 47, in_b = 5 -> out = 235, out_q = 9.
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/interrupt_mul.c",
            "-o",
            "tests/fixtures/interrupt_mul.hex",
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

    let (layout, _asm) = compile_fixture();
    let addr = |g: &str| {
        *layout
            .globals
            .get(g)
            .unwrap_or_else(|| panic!("no global {g}")) as usize
    };

    let hex = std::fs::read_to_string("tests/fixtures/interrupt_mul.hex").unwrap();
    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
    p.ram_mut()[addr("in_a")] = 47;
    p.ram_mut()[addr("in_b")] = 5;
    p.run(500_000);
    assert!(p.halted(), "program must SLEEP-halt");
    assert_eq!(p.ram()[addr("out")], 235, "out == 47 * 5");
    assert_eq!(
        p.ram()[addr("out_q")],
        9,
        "out_q == 47 / 5 (the remainder would be 2)"
    );
    assert_eq!(
        p.ram()[addr("isr_out")],
        0,
        "the ISR never ran, so isr_out stays 0"
    );
}
