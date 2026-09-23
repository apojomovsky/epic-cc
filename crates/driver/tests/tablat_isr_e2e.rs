//! epic-cc#532 regression: an ISR that performs a const (flash) read must
//! not corrupt the preempted main context's in-flight TBLRD value.
//!
//! `TBLRD*` leaves the flash byte in TABLAT and the consumer `MOVFF 0xFF5,
//! dst` is the very next instruction, so the byte is live across one
//! instruction boundary. ADR-013 saves TBLPTR for this same read sequence
//! but TABLAT was in no save set; before this fix a handler whose own const
//! read emits TBLRD resumed main against the handler's byte.
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
            "tests/fixtures/tablat_isr.c",
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

    // Both contexts must actually read flash, or this proves nothing.
    assert!(
        asm.matches("TBLRD").count() >= 2,
        "both main and the handler must emit a const read:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFF5"),
        "the const read's consumer moves TABLAT:\n{asm}"
    );

    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    // idx/isr_idx arrive via the fixture's init stores (epic-cc#561):
    // __start clears zero-initialized globals, so sim-side seeds would
    // not survive.

    // Interrupt at the instruction right after main's TBLRD*: TABLAT holds
    // main's 0x22 and the consumer has not run yet.
    let mut steps = 0;
    let mut fired = false;
    while steps < 4000 {
        p.step();
        steps += 1;
        if p.ram()[0xFF5] == 0x22 && p.ram()[sym("out")] == 0 {
            p.fire_interrupt();
            p.run(20_000);
            fired = true;
            break;
        }
    }
    assert!(fired, "never observed TABLAT holding main's byte");

    assert_eq!(
        p.ram()[sym("out")],
        0x22,
        "main's const read must survive a handler that reads flash too"
    );
    assert_eq!(
        p.ram()[sym("isr_out")],
        0x88,
        "the handler's own read is 0x88"
    );
}

#[test]
fn isr_const_read_preserves_the_preempted_tblrd_value() {
    run();
}
