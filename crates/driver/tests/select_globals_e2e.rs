//! Issue #147: a pointer select between two distinct global addresses (the
//! `ok_flag ? "PASS\n" : "FAIL\n"` shape) must compile and read the
//! selected global's first byte on both cores.
//!
//! Hand-computed expectations (sim sets `ok_flag` before run):
//!   - ok_flag = 1: out = 'P' (0x50)
//!   - ok_flag = 0: out = 'F' (0x46)
use std::collections::HashMap;
use std::process::Command;

fn layout_for_device(dev: &'static device::Device) -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/select_globals.c"),
        &driver::clang::Options::default(),
    );
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(dev, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = match dev.core {
        device::Core::Pic14 => isel::select(dev, &m, &addrs),
        device::Core::Pic18 => isel_pic18::select(dev, &m, &addrs),
        _ => panic!("unsupported core"),
    };
    let _ = match dev.core {
        device::Core::Pic14 => {
            let a = banking::assign_banks(dev, &asm);
            peephole::optimize(&a)
        }
        _ => asm,
    };
    layout
}

fn run_select_globals(device_name: &str, device: &'static device::Device) {
    let layout = layout_for_device(device);
    let ok_addr = *layout.globals.get("ok_flag").expect("ok_flag") as usize;
    let out_addr = *layout.globals.get("out").expect("out") as usize;

    let hex_path = format!("tests/fixtures/select_globals_{device_name}.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/select_globals.c",
            "-o",
            &hex_path,
            "--device",
            device_name,
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).expect("read hex");

    let check = |flag: u8, expected: u8| match device.core {
        device::Core::Pic14 => {
            let mut sim = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
            sim.ram_mut()[ok_addr] = flag;
            sim.run(200_000);
            assert_eq!(
                sim.ram()[out_addr],
                expected,
                "out {device_name} flag={flag}"
            );
            assert!(sim.halted(), "halted {device_name} flag={flag}");
        }
        device::Core::Pic18 => {
            let mut sim = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            sim.ram_mut()[ok_addr] = flag;
            sim.run(200_000);
            assert_eq!(
                sim.ram()[out_addr],
                expected,
                "out {device_name} flag={flag}"
            );
            assert!(sim.halted(), "halted {device_name} flag={flag}");
        }
        device::Core::Pic14e => panic!("pic14e core not implemented"),
    };
    check(1, b'P');
    check(0, b'F');
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn select_globals_runs_on_p16() {
    run_select_globals("p16f877a", &device::PIC16F877A);
}

#[test]
fn select_globals_runs_on_p18() {
    run_select_globals("p18f4550", &device::PIC18F4550);
}
