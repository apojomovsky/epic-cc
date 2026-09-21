//! epic-cc#493 regression: an ISR that seeds FSR1 must not corrupt the
//! preempted main context's in-flight FSR1 copy pointer.
//!
//! main's `g_storage = *h` lowers to an indirect-source memcpy: FSR1 is
//! seeded with the source pointer and read byte by byte, so the pointer is
//! live across the copy's instructions. epic-cc#477 added FSR1L/FSR1H to
//! every ISR prologue/epilogue; before that, an interrupt taken inside that
//! window and served by an ISR whose code also seeds FSR1 (the fixture's
//! handler copies its own struct) resumed main against the ISR's pointer.
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
            "tests/fixtures/fsr1_isr.c",
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

    // The handler must actually seed FSR1, or this test proves nothing.
    assert!(
        asm.contains("0xFE1"),
        "the ISR must seed/save FSR1 for this fixture to exercise the window:\n{asm}"
    );

    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    // The ISR's source struct carries a distinguishable pattern.
    for (i, b) in [0xAAu8, 0xBB, 0xCC, 0xDD].iter().enumerate() {
        p.ram_mut()[sym("isr_src") + i] = *b;
    }

    // Interrupt while main's copy is in flight: FSR1 holds main's source
    // pointer and PRODL is irrelevant here, so trigger on the seeded FSR1
    // high byte being main's (0x00 at this layout) and g_out still unset.
    let mut steps = 0;
    let mut fired = false;
    while steps < 4000 {
        p.step();
        steps += 1;
        if p.ram()[0xFE1] != 0 && p.ram()[sym("g_out")] == 0 {
            p.fire_interrupt();
            p.run(20_000);
            fired = true;
            break;
        }
    }
    assert!(
        fired,
        "never observed main mid-copy to inject the interrupt"
    );

    assert_eq!(
        p.ram()[sym("g_out")],
        0x55,
        "main's copy must survive an ISR that seeds FSR1 (g_out should be cb_impl's 0x55)"
    );
}

#[test]
fn isr_copy_preserves_the_preempted_fsr1_copy_pointer() {
    run();
}
