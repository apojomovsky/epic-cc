//! Bit-fields acceptance (PIC14 parity, #268): a struct with bit-fields,
//! written and read back through the same fields, on both cores. The
//! differential probe (crates/sdcc-parity) already passes; this pins the
//! behavior as a committed e2e test.
//!
//! in = 0x6D (01101101): f.a = in & 3 = 1, f.b = (in>>2) & 7 = 3,
//! f.c = (in>>5) & 7 = 3. out = f.a | (f.b<<2) | (f.c<<5) = 1 | 12 | 96 =
//! 109 = 0x6D. out2 = f.a + f.b + f.c = 7 (the read-back path).

use std::process::Command;

fn layout_for(device: &device::Device) -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/bitfields.c"),
        &driver::clang::Options::default(),
    );
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    alloc::allocate(device, &m, &callgraph::edges_text(&cg))
}

fn run_on(device: &device::Device, device_name: &str) {
    let layout = layout_for(device);
    let in_addr = *layout.globals.get("in").expect("in global") as usize;
    let out_addr = *layout.globals.get("out").expect("out global") as usize;
    let out2_addr = *layout.globals.get("out2").expect("out2 global") as usize;

    let hex_path = std::env::temp_dir().join(format!(
        "bitfields_{device_name}_{}.hex",
        std::process::id()
    ));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", device_name, "-o"])
        .arg(&hex_path)
        .arg("tests/fixtures/bitfields.c")
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let _ = std::fs::remove_file(&hex_path);

    match device.core {
        device::Core::Pic14 => {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
            p.ram_mut()[in_addr] = 0x6D;
            p.run(200_000);
            assert_eq!(p.ram()[out_addr], 0x6D, "out mismatch on {device_name}");
            assert_eq!(p.ram()[out2_addr], 7, "out2 mismatch on {device_name}");
            assert!(p.halted());
        }
        device::Core::Pic18 => {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            p.ram_mut()[in_addr] = 0x6D;
            p.run(200_000);
            assert_eq!(p.ram()[out_addr], 0x6D, "out mismatch on {device_name}");
            assert_eq!(p.ram()[out2_addr], 7, "out2 mismatch on {device_name}");
            assert!(p.halted());
        }
        other => panic!("bitfields: unsupported core {other:?}"),
    }
}

#[test]
fn bitfields_runs_on_p16() {
    run_on(&device::PIC16F877A, "p16f877a");
}

#[test]
fn bitfields_runs_on_p18() {
    run_on(&device::PIC18F4550, "p18f4550");
}
