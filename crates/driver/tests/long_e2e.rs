//! Milestone-12 "long" acceptance: the whole i32 surface — add via a
//! noinline call, mul, udiv, urem, sdiv, srem, const-count shl/lshr/ashr
//! (inline), icmp ult/ugt/ule, trunc/zext/sext (i32->i8/i16, i8/i16->i32),
//! and a struct with a long member via byval/sret ({i8,i32}, 6-byte
//! layout) — compiles through the whole driver pipeline and runs correctly
//! in the simulator. Acceptance: for in == 0x12345678 and sin == -19 the
//! hand-computed `out == 0x1634943A` and the machine halts.
//!
//! `out`, `in`, `sin`, `sp`, `g8`, `g16` are globals; their addresses are
//! read from the same alloc layout the driver used.
//!
//! Hand computation from the exact emitted IR (fixtures/long.c through
//! clang -O1; `in` = 0x12345678 = 305419896, `sin` = -19 = 0xFFFFFFED):
//!   main:
//!     %1 = load volatile i32 @in                  0x12345678
//!     %2 = tail call i32 @addm(%1, 5)             0x1234567D   (i32 add call)
//!     %3 = udiv i32 %1, 271                       0x113262     (__udiv_u32; 0x10F)
//!     %4 = mul i32 %3, 7                          0x7860AE     (__mul_u32)
//!     %5 = urem i32 %1, 270                       0xD8         (__urem_u32; 0x10E)
//!     %6 = load volatile i32 @sin                 0xFFFFFFED
//!     %7 = sdiv i32 %6, -3                        6            (__sdiv_i32)
//!     %8 = srem i32 %6, 3                         0xFFFFFFFF   (__srem_i32)
//!     %9 = tail call i32 @misc(%1, %4, %5, %6)    0x1634943A
//!   misc:
//!     %6  = add i32 %2(u), %1(m)                  0x7860AE + 0xD8 = 0x786186
//!     %7  = shl i32 %6, 3                         0x3C30C30
//!     %8  = lshr i32 %6, 1                        0x3C30C3
//!     %9  = or i32 %7, %8                         0x03FF3CF3
//!     %10 = ashr i32 %3(s), 4                     -19>>4 = -2 = 0xFFFFFFFE
//!     %11 = icmp ult i32 %0, 0x20000000           1 (0x12345678 < 0x20000000)
//!     %12 = zext i1 %11 to i32                    1
//!     %13 = icmp ugt i32 %0, 0x1000               1
//!     %14 = select i1 %13, 2, 0                   2
//!     %15 = icmp ult i32 %0, 0x12345679           1 (C `a <= 0x12345678`
//!          canonicalized to ult with k+1)
//!     %16 = select i1 %15, 4, 0                   4
//!     %17 = trunc i32 %0 to i8                    0x78
//!          store volatile i8 %17, @g8
//!     %18 = load volatile i8 @g8                  0x78
//!     %19 = zext i8 %18 to i32                    0x78
//!     %20 = load volatile i8 @g8                  0x78
//!     %21 = sext i8 %20 to i32                    0x78 (bit 7 clear)
//!     %22 = trunc i32 %3 to i16                   0xFFED (sin low half)
//!          store volatile i16 %22, @g16
//!     %23 = load volatile i16 @g16                0xFFED
//!     %24 = zext i16 %23 to i32                   0xFFED
//!     %25 = sext i16 %23 to i32                   0xFFFFFFED (bit 15 set)
//!          mkp(%0) -> %5 alloca, memcpy %5 -> @sp (6 bytes):
//!          sp.a = 0xAB, sp.b = 0x12345678
//!     %26 = tail call i32 @getb(byval @sp)        sp.b = 0x12345678
//!     %27 = or i32 %12, %14                       1 | 2 = 3
//!     %28 = or i32 %27, %16                       3 | 4 = 7
//!     %29 = add i32 %28, %10                      7 + 0xFFFFFFFE = 5
//!     %30 = add i32 %29, %9                       5 + 0x03FF3CF3 = 0x03FF3CF8
//!     %31 = add i32 %30, %19                      0x03FF3CF8 + 0x78 = 0x03FF3D70
//!     %32 = add i32 %31, %21                      0x03FF3D70 + 0x78 = 0x03FF3DE8
//!     %33 = add i32 %32, %24                      0x03FF3DE8 + 0xFFED = 0x04003DD5
//!     %34 = add i32 %33, %25                      0x04003DD5 + 0xFFFFFFED = 0x04003DC2
//!     %35 = add i32 %34, %26                      0x04003DC2 + 0x12345678 = 0x1634943A
//! The clang-side shape notes (why the volatile g8/g16 round-trips are
//! needed to keep genuine trunc/zext/sext IR ops, why the udiv/urem
//! divisors are 0x10F/0x10E, why main keeps exactly 9 i32 locals) live in
//! fixtures/long.c.

use std::collections::HashMap;
use std::process::Command;

fn long_layout() -> alloc::AllocLayout {
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
            "tests/fixtures/long.c",
        ])
        .output()
        .expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();

    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel::select(&device::PIC16F877A, &m, &addrs);
    let asm = banking::assign_banks(&device::PIC16F877A, &asm);
    let _ = peephole::optimize(&asm);
    layout
}

#[test]
fn long_runs_correctly() {
    let layout = long_layout();
    let in_addr = *layout.globals.get("in").expect("in global") as usize;
    let sin_addr = *layout.globals.get("sin").expect("sin global") as usize;
    let out_addr = *layout.globals.get("out").expect("out global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["tests/fixtures/long.c", "-o", "tests/fixtures/long.hex", "--device", "p16f877a"])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/long.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    let val: u32 = 0x12345678; // in (little-endian i32)
    p.ram_mut()[in_addr] = (val & 0xFF) as u8;
    p.ram_mut()[in_addr + 1] = ((val >> 8) & 0xFF) as u8;
    p.ram_mut()[in_addr + 2] = ((val >> 16) & 0xFF) as u8;
    p.ram_mut()[in_addr + 3] = ((val >> 24) & 0xFF) as u8;
    let sval: u32 = (-19i32) as u32; // sin = -19 (little-endian i32)
    p.ram_mut()[sin_addr] = (sval & 0xFF) as u8;
    p.ram_mut()[sin_addr + 1] = ((sval >> 8) & 0xFF) as u8;
    p.ram_mut()[sin_addr + 2] = ((sval >> 16) & 0xFF) as u8;
    p.ram_mut()[sin_addr + 3] = ((sval >> 24) & 0xFF) as u8;
    p.run(5_000_000);
    let got = (p.ram()[out_addr] as u32)
        | ((p.ram()[out_addr + 1] as u32) << 8)
        | ((p.ram()[out_addr + 2] as u32) << 16)
        | ((p.ram()[out_addr + 3] as u32) << 24);
    assert_eq!(got, 0x1634943A, "out == hand-computed 0x1634943A for in == 0x12345678, sin == -19");
    assert!(p.halted());
}
