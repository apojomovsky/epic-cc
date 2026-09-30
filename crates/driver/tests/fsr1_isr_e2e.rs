//! epic-cc#493 regression: an ISR that seeds the FSRs must not corrupt the
//! preempted main context's in-flight copy pointer.
//!
//! main's `g_storage = *h` lowers to an indirect-source walk: FSR0 is
//! seeded with the source pointer and advanced byte by byte, so the
//! pointer is live across the copy's instructions. Since epic-cc#723 the
//! load/store temp is folded away, so main holds no FSR1 loop; the
//! fixture's handler still copies its own struct through an FSR0/FSR1
//! loop, and epic-cc#477's FSR save/restore in every ISR prologue and
//! epilogue is what lets the preempted walk resume. An interrupt taken
//! inside main's window and served without that restore would resume
//! main against the ISR's pointer.
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

    // Interrupt while main's copy is in flight: store_handle seeds FSR0
    // with h's frame slot (0x10) for its walk, so trigger only when FSR0L
    // holds exactly that. A bare nonzero check also fires in `__start`,
    // which seeds FSR0 for its zero-clear loop while g_out is still unset,
    // and the test would pass without ever preempting the copy. The exact
    // match is precise for this layout: pre-window FSR0L takes only 0x00
    // then the clear loops' 0x2C-and-up range, so 0x10 fires at the copy's
    // seed step and nowhere earlier.
    let mut steps = 0;
    let mut fired = false;
    while steps < 4000 {
        p.step();
        steps += 1;
        if p.ram()[0xFE9] == 0x10 && p.ram()[sym("g_out")] == 0 {
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
