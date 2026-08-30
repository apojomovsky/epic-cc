//! Issue #148: string literal as pointer call argument must compile and read correctly.
use std::collections::HashMap;
use std::process::Command;

fn layout_for_device(dev: &'static device::Device) -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/str_literal.c"),
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

#[test]
fn str_literal_compiles_to_hex_on_both_devices() {
    for (name, _dev) in [
        ("p16f877a", &device::PIC16F877A),
        ("p16f887", &device::PIC16F887),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
            .args([
                "tests/fixtures/str_literal.c",
                "-o",
                &format!("tests/fixtures/str_literal_{name}.hex"),
                "--device",
                name,
            ])
            .output()
            .expect("run driver");
        assert!(
            out.status.success(),
            "driver for {name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let hex =
            std::fs::read_to_string(format!("tests/fixtures/str_literal_{name}.hex")).unwrap();
        assert!(hex.contains(':'), "hex for {name} looks empty");
    }
}

#[test]
fn str_literal_bytes_are_readable_on_pic14() {
    let layout = layout_for_device(&device::PIC16F877A);
    let g_tx = *layout.globals.get("g_tx").expect("g_tx") as usize;
    let g_tx_len = *layout.globals.get("g_tx_len").expect("g_tx_len") as usize;
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/str_literal.c",
            "-o",
            "tests/fixtures/str_literal_run.hex",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string("tests/fixtures/str_literal_run.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(500_000);
    let expected = b"epic-serial ready\r\n";
    assert_eq!(p.ram()[g_tx_len], expected.len() as u8, "g_tx_len");
    for (i, b) in expected.iter().enumerate() {
        assert_eq!(p.ram()[g_tx + i], *b, "g_tx[{i}]");
    }
    assert!(p.halted());
}
