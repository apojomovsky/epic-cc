//! #72 acceptance: const array and single globals of a named struct type
//! decode to their initializer bytes and read correctly through the flash
//! readers on both PIC14 (p16f877a) and PIC18 (p18f4550).

use std::process::Command;

fn layout_for(device: &device::Device, fixture: &str) -> alloc::AllocLayout {
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
            resdir.as_str(),
            "-I",
            "tests/fixtures",
            fixture,
            "-o",
            "-",
        ])
        .output()
        .expect("run clang");
    assert!(
        ll.status.success(),
        "clang failed: {}",
        String::from_utf8_lossy(&ll.stderr)
    );
    let ll_text = String::from_utf8(ll.stdout).unwrap();
    {
        let m0 = irparse::parse_ll(&ll_text);
        let gtbl = m0
            .globals
            .iter()
            .find(|g| g.name == "tbl")
            .expect("tbl global");
        assert_eq!(
            gtbl.bytes,
            vec![11, 22, 33, 44, 55, 66, 77, 88],
            "irparse bytes for tbl: {:?}",
            gtbl.bytes
        );
        let gsingle = m0
            .globals
            .iter()
            .find(|g| g.name == "single")
            .expect("single");
        assert_eq!(
            gsingle.bytes,
            vec![99, 101, 103, 105],
            "single bytes {:?}",
            gsingle.bytes
        );
    }
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(device, &m, &callgraph::edges_text(&cg));
    layout
}

fn run_on_device(device_str: &str, device: &device::Device) {
    let layout = layout_for(device, "tests/fixtures/named_struct_globals.c");
    let addr = |n: &str| *layout.globals.get(n).expect(n) as usize;

    let hex_name = format!("tests/fixtures/named_struct_globals_{device_str}.hex");
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/named_struct_globals.c",
            "-o",
            &hex_name,
            "--device",
            device_str,
        ])
        .output()
        .expect("run driver");
    assert!(
        output.status.success(),
        "driver --device {device_str} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let hex = std::fs::read_to_string(&hex_name).unwrap();

    if device_str == "p16f877a" {
        let prog = pic14_sim::parse_hex(&hex);
        let mut p = pic14_sim::Pic14::new(prog.clone());
        p.ram_mut()[addr("idx")] = 0;
        p.run(200_000);
        assert!(p.halted(), "PIC14 idx=0 should halt");
        assert_eq!(p.ram()[addr("out0")], 11, "out0 tbl[0][0]");
        assert_eq!(p.ram()[addr("out1")], 22, "out1 tbl[0][1]");
        assert_eq!(p.ram()[addr("out2")], 77, "out2 tbl[1][2]");
        assert_eq!(p.ram()[addr("out3")], 44, "out3 tbl[idx][3] idx=0");
        assert_eq!(p.ram()[addr("out4")], 11, "out4 tbl[0].a");
        assert_eq!(p.ram()[addr("out5")], 66, "out5 tbl[1].b");
        assert_eq!(p.ram()[addr("out6")], 33, "out6 tbl[idx].c idx=0");
        assert_eq!(p.ram()[addr("out7")], 44, "out7 tbl[idx].d idx=0");
        assert_eq!(p.ram()[addr("out_s0")], 99, "single byte0");
        assert_eq!(p.ram()[addr("out_s1")], 101, "single b");
        assert_eq!(p.ram()[addr("out_s2")], 103, "single byte2");
        assert_eq!(p.ram()[addr("out_s3")], 105, "single d");

        let mut p = pic14_sim::Pic14::new(prog);
        p.ram_mut()[addr("idx")] = 1;
        p.run(200_000);
        assert!(p.halted());
        assert_eq!(p.ram()[addr("out3")], 88, "out3 idx=1");
        assert_eq!(p.ram()[addr("out6")], 77, "out6 idx=1");
        assert_eq!(p.ram()[addr("out7")], 88, "out7 idx=1");
    } else {
        let prog = pic14_sim::parse_hex_pic18(&hex);
        let mut p = pic14_sim::Pic18::new(prog.clone());
        p.ram_mut()[addr("idx")] = 0;
        p.run(200_000);
        assert!(p.halted(), "PIC18 idx=0 should halt");
        assert_eq!(p.ram()[addr("out0")], 11, "PIC18 out0");
        assert_eq!(p.ram()[addr("out1")], 22);
        assert_eq!(p.ram()[addr("out2")], 77);
        assert_eq!(p.ram()[addr("out3")], 44);
        assert_eq!(p.ram()[addr("out4")], 11);
        assert_eq!(p.ram()[addr("out5")], 66);
        assert_eq!(p.ram()[addr("out6")], 33);
        assert_eq!(p.ram()[addr("out7")], 44);
        assert_eq!(p.ram()[addr("out_s0")], 99);
        assert_eq!(p.ram()[addr("out_s1")], 101);
        assert_eq!(p.ram()[addr("out_s2")], 103);
        assert_eq!(p.ram()[addr("out_s3")], 105);

        let mut p = pic14_sim::Pic18::new(prog);
        p.ram_mut()[addr("idx")] = 1;
        p.run(200_000);
        assert!(p.halted());
        assert_eq!(p.ram()[addr("out3")], 88);
        assert_eq!(p.ram()[addr("out6")], 77);
        assert_eq!(p.ram()[addr("out7")], 88);
    }

    let _ = std::fs::remove_file(&hex_name);
}

#[test]
fn named_struct_globals_pic14() {
    run_on_device("p16f877a", &device::PIC16F877A);
}

#[test]
fn named_struct_globals_pic18() {
    run_on_device("p18f4550", &device::PIC18F4550);
}
