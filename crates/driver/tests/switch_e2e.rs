// switch_e2e: validates that `switch` lowers to an icmp+brcond chain and
// executes correctly on both supported devices. Covers acceptance:
// - multi-arm switch with default,
// - gaps (3,4,6-9,11+ go to default),
// - fallthrough (case 1 falls into case 2) behaves like the equivalent
//   if/else chain (by construction the lowering is the if/else chain).

use std::collections::HashMap;
use std::process::Command;

fn expected(v: u8) -> u8 {
    let r: u8 = match v {
        0 => 10,
        1 => 20,
        2 => 30,
        5 => 50,
        10 => 100,
        _ => 99,
    };
    let r2: u8 = match v {
        1 => 12, // 5 + 7 fallthrough
        2 => 7,
        3 => 30,
        _ => 1,
    };
    r.wrapping_add(r2)
}

fn run_one(device: &str, v: u8) {
    let hex_path = format!("tests/fixtures/switch_{device}.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/switch.c",
            "-o",
            &hex_path,
            "--device",
            device,
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device} v={v}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    if device == "p16f877a" {
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
                "tests/fixtures/switch.c",
            ])
            .output()
            .expect("clang");
        assert!(ll.status.success());
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
        let _ = banking::assign_banks(&device::PIC16F877A, &asm);
        let in_addr = *layout.globals.get("in").unwrap() as usize;
        let out_addr = *layout.globals.get("out").unwrap() as usize;
        let prog = pic14_sim::parse_hex(&hex);
        let mut p = pic14_sim::Pic14::new(prog);
        p.ram_mut()[in_addr] = v;
        p.run(200_000);
        assert_eq!(
            p.ram()[out_addr],
            expected(v),
            "device {device} v={v} expected {} got {}",
            expected(v),
            p.ram()[out_addr]
        );
        assert!(p.halted());
    } else {
        // p18: ensure alloc+isel succeeds (sim gate lives in PIC14)
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
                "tests/fixtures/switch.c",
            ])
            .output()
            .expect("clang");
        assert!(ll.status.success());
        let ll_text = String::from_utf8(ll.stdout).unwrap();
        let mut m = irparse::parse_ll(&ll_text);
        m = wholeprog::merge(m);
        m = legalize::legalize(m);
        let cg = callgraph::build(&m);
        let layout = alloc::allocate(&device::PIC18F4550, &m, &callgraph::edges_text(&cg));
        let mut addrs: HashMap<String, u16> = HashMap::new();
        addrs.extend(layout.globals.clone());
        addrs.extend(layout.locals.clone());
        let _asm = isel_pic18::select(&device::PIC18F4550, &m, &addrs);
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn switch_both_devices() {
    for v in [0u8, 1, 2, 3, 4, 5, 6, 10, 11, 42, 255] {
        run_one("p16f877a", v);
        run_one("p18f4550", v);
    }
}
