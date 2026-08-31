//! Issue #148: string literal as pointer call argument must compile and read correctly.
use std::collections::HashMap;
use std::process::Command;

fn layout_for_device(dev: &'static device::Device) -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let header_dir = {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tid = format!("{:?}", std::thread::current().id());
        let dir = std::env::temp_dir()
            .join(format!("str-literal-{}-{}-{}", std::process::id(), tid, ns))
            .join("include");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stdint.h"), driver::stdint_h::STDINT_H).unwrap();
        std::fs::write(dir.join("stdbool.h"), driver::stdbool_h::STDBOOL_H).unwrap();
        std::fs::write(dir.join("stddef.h"), driver::stddef_h::STDDEF_H).unwrap();
        std::fs::write(dir.join("string.h"), driver::string_h::STRING_H).unwrap();
        std::fs::write(dir.join("stdlib.h"), driver::stdlib_h::STDLIB_H).unwrap();
        std::fs::write(dir.join("epic-cc.h"), driver::epic_cc_h::EPIC_CC_H).unwrap();
        dir
    };
    let opts = driver::clang::Options {
        header_dir: Some(header_dir.clone()),
        ..Default::default()
    };
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/str_literal.c"),
        &opts,
    );
    let mut m = irparse::parse_ll(&ll_text);
    // Mirror driver/main.rs: when the source includes string.h, the
    // string.c translation unit is appended before wholeprog.
    let src_text = std::fs::read_to_string("tests/fixtures/str_literal.c").unwrap_or_default();
    if src_text
        .lines()
        .any(|l| l.contains("#include") && l.contains("string.h"))
    {
        let c_path = header_dir.parent().unwrap().join("__epic_string.c");
        std::fs::write(&c_path, driver::string_c::STRING_C).unwrap();
        let mut sm = irparse::parse_ll(&driver::clang::compile_to_stdout(
            &clang, &resdir, &c_path, &opts,
        ));
        m.funcs.extend(sm.funcs.drain(..));
        m.globals.extend(sm.globals.drain(..));
        m.module_asm.extend(sm.module_asm.drain(..));
        let _ = std::fs::remove_file(&c_path);
    }
    let _ = std::fs::remove_dir_all(header_dir.parent().unwrap());
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
