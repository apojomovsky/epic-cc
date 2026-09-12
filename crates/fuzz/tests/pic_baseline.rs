//! PIC baseline differential gate (P8, docs/37): the PIC14E gate's corpora
//! threaded through `&device::PIC12F509` and the 509's 41-byte GPR file.
//! The fast subsets (seeds 0..8) run as normal tests; the full corpora run
//! under `--ignored` and must be clean. No float corpus: P7 soft-float is
//! a documented non-goal on this core (#386).

use alloc;
use callgraph;
use device::PIC12F509;
use fuzz::{
    generate_baseline, generate_ir_baseline, generate_signed_baseline, run_differential,
    run_ir_differential, IrProgram,
};
use ir;
use legalize;
use wholeprog;

#[test]
fn pic_baseline_integer_fast_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_baseline(seed);
        match run_differential(&prog, &PIC12F509) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic-baseline integer seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 integer seeds clean");
}

#[test]
fn pic_baseline_signed_fast_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_signed_baseline(seed);
        match run_differential(&prog, &PIC12F509) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic-baseline signed seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 signed seeds clean");
}

#[test]
fn pic_baseline_ir_fast_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..8 {
        let prog = generate_ir_baseline(seed);
        match run_ir_differential(&prog, &PIC12F509) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic-baseline IR seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 8, "all 8 IR seeds clean");
}

#[test]
#[ignore = "full 200-seed pic-baseline integer corpus (slow)"]
fn pic_baseline_full_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..200 {
        let prog = generate_baseline(seed);
        match run_differential(&prog, &PIC12F509) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic-baseline integer seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 200, "all 200 integer seeds clean");
}

#[test]
#[ignore = "full 50-seed pic-baseline signed corpus (slow)"]
fn pic_baseline_signed_corpus_differential_clean() {
    let mut clean = 0usize;
    for seed in 0..50 {
        let prog = generate_signed_baseline(seed);
        match run_differential(&prog, &PIC12F509) {
            Ok(_) => clean += 1,
            Err(e) => panic!("pic-baseline signed seed {seed} not differential-clean: {e}"),
        }
    }
    assert_eq!(clean, 50, "all 50 signed seeds clean");
}

/// docs/37 D-2's ordering hazard end to end: INDF accesses through a
/// runtime pointer interleave with direct accesses to a bank-1 global, in
/// both orders. The pad count calibrates against the real allocator so
/// @b1 lands in bank 1: without it a placement change would silently
/// reduce this to a common-RAM test.
#[test]
fn pic_baseline_d2_banked_direct_and_indf_ordering_clean() {
    let mut placed = None;
    for pads in 18..=25 {
        let prog = d2_program(pads);
        let m = legalize::legalize(wholeprog::merge(ir::parse(&prog.ir_text)));
        let cg = callgraph::build(&m);
        let layout = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            alloc::allocate(&PIC12F509, &m, &callgraph::edges_text(&cg))
        }));
        let Ok(layout) = layout else { continue };
        let b1 = *layout.globals.get("b1").expect("b1 in the alloc map");
        if b1 >= 0x30 {
            placed = Some(prog);
            break;
        }
    }
    let prog = placed.expect("no pad count places b1 in bank 1 on the p12f509");
    match run_ir_differential(&prog, &PIC12F509) {
        Ok(_) => {}
        Err(e) => panic!("pic-baseline D-2 ordering program not differential-clean: {e}"),
    }
}

/// The D-2 ordering program at a given pad count. The pads (each kept
/// alive by a store) walk placement through common (0x07-0x0F) and bank 0
/// (0x10-0x1F) so @b1 can land in bank 1.
fn d2_program(pads: usize) -> IrProgram {
    let mut pad_globals = String::new();
    let mut pad_stores = String::new();
    for i in 0..pads {
        pad_globals.push_str(&format!("global p{i} i8\n"));
        pad_stores.push_str(&format!("    store i8 0 @p{i}\n"));
    }
    let ir_text = format!(
        "global in i16\n{pad_globals}global b1 i8\nglobal checksum i8\n\
         fn main(void) ()\n  block entry:\n\
         %1 = load i16 @in\n\
         %2 = trunc i16 %1 to i8\n\
         {pad_stores}\
         %3 = inttoptr i16 8 to ptr\n\
         store i8 %2 %3\n\
         %4 = load i8 %3\n\
         store i8 42 @b1\n\
         %5 = load i8 @b1\n\
         store i8 %5 %3\n\
         %6 = load i8 %3\n\
         %7 = add i8 %4 5\n\
         %8 = add i8 %6 %7\n\
         %9 = add i8 %8 %5\n\
         store i8 %9 @checksum\n\
         ret void\n"
    );
    let c_twin = "\
typedef unsigned char u8; typedef unsigned short u16; typedef unsigned int u32;
volatile u16 in;
volatile u8 checksum;
void main(void) {
  u8 v = (u8)in;
  u8 y = (u8)(v + 5 + 42 + 42);
  checksum = y;
}
";
    IrProgram {
        ir_text,
        inputs: vec![fuzz::Input {
            name: "in".to_string(),
            value: 0xBEEF,
            width: 16,
            is_float: false,
        }],
        checksum_name: "checksum".to_string(),
        seed: 330,
        c_twin: c_twin.to_string(),
    }
}
