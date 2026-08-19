//! Issue #5 acceptance: const struct globals decode into flat flash bytes
//! and read correctly through the RETLW table readers at runtime.
//!
//! Hand-computed expectations (sim sets `idx` = 0 before run):
//!   - C1 = { 'A', pad, 0x1234 } -> flash [0x41, 0x00, 0x34, 0x12]
//!   - CARR[0] = { 'D', 0x1111 }, CARR[1] = { 'E', 0x2222 }
//!   - byval_c1(C1)         -> out_a = 0x41, out_b = 0x1234
//!   - byval_elem1(CARR[1]) -> out_a2 = 0x45, out_b2 = 0x2222
//!   - byval_var(CARR[idx]) -> out_a3 = 0x44, out_b3 = 0x1111 (idx=0)
//!   - ((u8*)&C1)[idx]      -> out_m0 = 0x41 (byte 0 = C1.a)
//!   - ((u8*)&C1)[idx + 2]  -> out_m1 = 0x34 (byte 2 = C1.b low byte)

use std::collections::HashMap;
use std::process::Command;

fn const_struct_layout() -> alloc::AllocLayout {
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
            "tests/fixtures/const_struct.c",
        ])
        .output()
        .expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();

    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let mut addrs: HashMap<String, u16> = HashMap::new();
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel::select(&device::PIC16F877A, &m, &addrs);
    let asm = banking::assign_banks(&device::PIC16F877A, &asm);
    let _ = peephole::optimize(&asm);
    layout
}

#[test]
fn const_struct_reads_run_correctly() {
    let layout = const_struct_layout();
    let addr = |n: &str| *layout.globals.get(n).expect(n) as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_driver"))
        .args(["tests/fixtures/const_struct.c", "tests/fixtures/const_struct.hex"])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/const_struct.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[addr("idx")] = 0; // CARR[idx] -> element 0
    p.run(200_000);
    assert!(p.halted());

    assert_eq!(p.ram()[addr("out_a")], 0x41, "out_a = C1.a");
    assert_eq!(p.ram()[addr("out_a2")], 0x45, "out_a2 = CARR[1].a");
    assert_eq!(p.ram()[addr("out_a3")], 0x44, "out_a3 = CARR[idx].a (idx=0)");
    let b = |p: &pic14_sim::Pic14, n: &str| {
        let a = addr(n);
        u16::from(p.ram()[a]) | (u16::from(p.ram()[a + 1]) << 8)
    };
    assert_eq!(b(&p, "out_b"), 0x1234, "out_b = C1.b");
    assert_eq!(b(&p, "out_b2"), 0x2222, "out_b2 = CARR[1].b");
    assert_eq!(b(&p, "out_b3"), 0x1111, "out_b3 = CARR[idx].b (idx=0)");
    assert_eq!(p.ram()[addr("out_m0")], 0x41, "out_m0 = ((u8*)&C1)[0]");
    assert_eq!(p.ram()[addr("out_m1")], 0x34, "out_m1 = ((u8*)&C1)[2]");

    // Second pass with idx = 1: the nonzero element-stride path (scale-4
    // term) and the byte reads at offset 1/3. Reset the machine (the first
    // run ended in SLEEP, so a continued run would not re-enter main).
    let mut p = pic14_sim::Pic14::new(prog.clone());
    p.ram_mut()[addr("idx")] = 1;
    p.run(200_000);
    assert!(p.halted());
    assert_eq!(p.ram()[addr("out_a3")], 0x45, "out_a3 = CARR[idx].a (idx=1)");
    assert_eq!(b(&p, "out_b3"), 0x2222, "out_b3 = CARR[idx].b (idx=1)");
    assert_eq!(p.ram()[addr("out_m0")], 0x00, "out_m0 = ((u8*)&C1)[1] (padding byte)");
    assert_eq!(p.ram()[addr("out_m1")], 0x12, "out_m1 = ((u8*)&C1)[3] (C1.b high byte)");
}
