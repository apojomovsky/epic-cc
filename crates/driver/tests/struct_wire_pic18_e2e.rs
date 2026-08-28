//! #166 acceptance: PIC18 struct layout is the XC8 wire format. The mixed
//! 8/16-bit descriptors keep their natural byte-aligned sizes (9 and 7), the
//! raw RAM image matches the USB wire bytes, and field reads agree with
//! byte-pointer reads. Compiled with the driver binary on p18f4550 and
//! checked in the Pic18 simulator.

use std::process::Command;

fn layout_for(device: &device::Device, fixture: &str) -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new(fixture),
        &driver::clang::Options {
            includes: vec!["tests/fixtures".to_string()],
            packed_structs: true,
            ..Default::default()
        },
    );
    {
        let m0 = irparse::parse_ll(&ll_text);
        let gcfg = m0
            .globals
            .iter()
            .find(|g| g.name == "cfg")
            .expect("cfg global");
        assert_eq!(gcfg.size, 9, "configuration_descriptor wire size");
        let gep = m0
            .globals
            .iter()
            .find(|g| g.name == "ep")
            .expect("ep global");
        assert_eq!(gep.size, 14, "two endpoint descriptors stride 7");
    }
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(device, &m, &callgraph::edges_text(&cg));
    layout
}

#[test]
fn pic18_struct_wire_format() {
    let fixture = "tests/fixtures/struct_wire_pic18.c";
    let layout = layout_for(&device::PIC18F4550, fixture);
    let addr = |n: &str| *layout.globals.get(n).expect(n) as usize;

    let hex_name = "tests/fixtures/struct_wire_pic18_p18f4550.hex";
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([fixture, "-o", hex_name, "--device", "p18f4550"])
        .output()
        .expect("run driver");
    assert!(
        output.status.success(),
        "driver failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let hex = std::fs::read_to_string(hex_name).unwrap();
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(200_000);
    assert!(p.halted(), "Pic18 sim should halt");

    // The RAM image of cfg is the USB wire format: 0x22's high byte sits at
    // offset 3, so wTotalLength occupies bytes 2..4 with no padding before.
    let cfg = addr("cfg");
    assert_eq!(
        &p.ram()[cfg..cfg + 9],
        &[9, 2, 0x22, 0x00, 1, 1, 0, 0x80, 0x32],
        "configuration_descriptor wire bytes"
    );
    // ep[1] starts at byte 7 (stride 7, not the align-2 stride 8).
    let ep = addr("ep");
    assert_eq!(
        &p.ram()[ep..ep + 14],
        &[
            7, 5, 0x81, 3, 0x40, 0x00, 1, // ep[0]
            7, 0, 0, 0, 0x10, 0x00, 0, // ep[1]
        ],
        "endpoint_descriptor array wire bytes"
    );

    // Field reads agree with the byte image.
    assert_eq!(p.ram()[addr("out_w")], 0x22, "out_w low byte");
    assert_eq!(p.ram()[addr("out_w") + 1], 0x00, "out_w high byte");
    assert_eq!(p.ram()[addr("out_b2")], 0x22, "byte read at offset 2");
    assert_eq!(p.ram()[addr("out_b3")], 0x00, "byte read at offset 3");
    assert_eq!(p.ram()[addr("out_b8")], 0x32, "byte read at offset 8");
    assert_eq!(
        p.ram()[addr("out_sum")],
        9 + 2 + 0x22 + 1 + 1 + 0 + 0x80 + 0x32,
        "sum of the raw cfg bytes"
    );
    assert_eq!(p.ram()[addr("out_img7")], 7, "ep[1].bLength at byte 7");
    assert_eq!(p.ram()[addr("out_img11")], 0x10, "ep[1].w low byte at 11");
    assert_eq!(p.ram()[addr("out_epw")], 0x10, "out_epw low byte");
    assert_eq!(p.ram()[addr("out_epw") + 1], 0x00, "out_epw high byte");

    let _ = std::fs::remove_file(hex_name);
}
