//! epic-cc#477 regression: an ISR that multiplies must not corrupt the
//! preempted main context's in-flight PRODL/PRODH product.
//!
//! The `__mul_*` routines write MULWF and read PRODL/PRODH several
//! instructions later, so the product is live across an interruptible
//! window. Before this fix the compat ISR saved neither byte, and an
//! interrupt taken inside that window made main resume against the ISR's
//! product: a silent wrong answer (out == the ISR's result).
use std::collections::HashMap;
use std::process::Command;

fn run() {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("clang env");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("resdir env");
    let ll = Command::new(&clang)
        .args([
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-g",
            "-resource-dir",
            &resdir,
            "-o",
            "-",
            "tests/fixtures/prod_isr.c",
        ])
        .output()
        .expect("run clang");
    assert!(
        ll.status.success(),
        "clang: {}",
        String::from_utf8_lossy(&ll.stderr)
    );
    let mut m = irparse::parse_ll(&String::from_utf8(ll.stdout).unwrap());
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&device::PIC18F4550, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel_pic18::select_with_locs(
        &device::PIC18F4550,
        &m,
        &addrs,
        layout.isr_low_save,
        layout.isr_save,
        layout.isr_hi_save,
    )
    .0;
    let sym = |n: &str| *addrs.get(n).unwrap_or_else(|| panic!("no {n}")) as usize;

    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    // Inputs arrive via the fixture's init stores (epic-cc#561): __start
    // clears zero-initialized globals, so sim-side seeds would not survive.

    // Run until the routine has written main's first partial product to
    // PRODL and main has not yet stored its result, then interrupt there:
    // the window where PROD holds main's live value.
    let main_product = 47u16.wrapping_mul(5) & 0xFF;
    let mut steps = 0;
    while steps < 400 {
        p.step();
        steps += 1;
        if p.ram()[0xFF3] == main_product as u8 && p.ram()[sym("out")] == 0 {
            p.fire_interrupt();
            p.run(4000);
            break;
        }
    }
    assert_eq!(
        p.ram()[sym("out")],
        235,
        "main's product must survive an ISR that multiplies (got {} from the ISR's {})",
        p.ram()[sym("out")],
        p.ram()[sym("isr_out")],
    );
    assert_eq!(
        p.ram()[sym("isr_out")],
        21,
        "the ISR's own product must be 21"
    );
}

#[test]
fn isr_multiply_preserves_the_preempted_product() {
    run();
}
