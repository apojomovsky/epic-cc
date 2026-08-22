//! CC-2 acceptance: the freestanding headers (`stdint.h`, `stdbool.h`,
//! `stddef.h`, `string.h`) and the `string.h` implementation the driver links
//! in when a source includes it, compiled through the whole pipeline and run
//! on both cores' simulators.
//!
//! `fixtures/cc2_string.c` exercises every implemented `<string.h>` entry
//! point and sums one point per passing check into `out`; the expected total
//! is hand-computed in the fixture. `memmove` gets overlapping ranges, the
//! check that pins the back-to-front copy and the pointer comparison
//! selecting it.

use std::process::Command;

fn header_dir() -> std::path::PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tid = format!("{:?}", std::thread::current().id());
    let dir = std::env::temp_dir()
        .join(format!("cc2-test-{}-{}-{}", std::process::id(), tid, ns))
        .join("include");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("stdint.h"), driver::stdint_h::STDINT_H).unwrap();
    std::fs::write(dir.join("stdbool.h"), driver::stdbool_h::STDBOOL_H).unwrap();
    std::fs::write(dir.join("stddef.h"), driver::stddef_h::STDDEF_H).unwrap();
    std::fs::write(dir.join("string.h"), driver::string_h::STRING_H).unwrap();
    std::fs::write(dir.join("epic-cc.h"), driver::epic_cc_h::EPIC_CC_H).unwrap();
    dir
}

fn compile_ll(clang: &str, resdir: &str, hdir: &std::path::Path, src: &str) -> String {
    let out = Command::new(clang)
        .args([
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-resource-dir",
            resdir,
            "-I",
            hdir.to_str().unwrap(),
            "-o",
            "-",
            src,
        ])
        .output()
        .expect("run clang");
    assert!(
        out.status.success(),
        "clang {src}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Rebuild the address map the driver computes for `fixture`, mirroring
/// `main.rs`: the extra `string.h` translation unit is appended to the user's
/// module before the whole-program stages run.
fn layout_for(device: &device::Device, fixture: &str) -> alloc::AllocLayout {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let hdir = header_dir();
    let mut m = irparse::parse_ll(&compile_ll(&clang, &resdir, &hdir, fixture));

    let src_text = std::fs::read_to_string(fixture).unwrap_or_default();
    if src_text
        .lines()
        .any(|l| l.contains("#include") && l.contains("string.h"))
    {
        let c_path = hdir.parent().unwrap().join("__epic_string.c");
        std::fs::write(&c_path, driver::string_c::STRING_C).unwrap();
        let mut sm = irparse::parse_ll(&compile_ll(
            &clang,
            &resdir,
            &hdir,
            c_path.to_str().unwrap(),
        ));
        m.funcs.extend(sm.funcs.drain(..));
        m.globals.extend(sm.globals.drain(..));
        m.module_asm.extend(sm.module_asm.drain(..));
    }

    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    alloc::allocate(device, &m, &callgraph::edges_text(&cg))
}

fn run_cc2(device_name: &str, device: &device::Device) {
    let fixture = "tests/fixtures/cc2_string.c";
    let hex_path = format!("tests/fixtures/cc2_string_{device_name}.hex");
    let layout = layout_for(device, fixture);
    let in_addr = *layout.globals.get("in").expect("in") as usize;
    let out_addr = *layout.globals.get("out").expect("out") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([fixture, "-o", &hex_path, "--device", device_name])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {device_name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).expect("read hex");

    // in = 7, so every check passing sums to 26 (see the fixture).
    let expected: u8 = 26;
    match device.core {
        device::Core::Pic14 => {
            let mut sim = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
            sim.ram_mut()[in_addr] = 7;
            sim.run(200_000);
            assert_eq!(sim.ram()[out_addr], expected, "out for {device_name} in=7");
            assert!(sim.halted(), "halted {device_name}");
        }
        device::Core::Pic18 => {
            let mut sim = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            sim.ram_mut()[in_addr] = 7;
            sim.run(200_000);
            assert_eq!(sim.ram()[out_addr], expected, "out for {device_name} in=7");
            assert!(sim.halted(), "halted {device_name}");
        }
    }
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn cc2_string_runs_on_p16() {
    run_cc2("p16f877a", &device::PIC16F877A);
}

#[test]
fn cc2_string_runs_on_p18() {
    run_cc2("p18f4550", &device::PIC18F4550);
}

#[test]
fn cc2_headers_compile_without_string() {
    // The type headers must stand alone: a source that never includes
    // <string.h> gets no extra translation unit linked in.
    let src = r#"
        #include <stdint.h>
        #include <stdbool.h>
        #include <stddef.h>
        volatile uint8_t in;
        volatile uint8_t out;
        void main(void) { bool b = true; size_t n = 1; uint16_t x = in; out = b ? (uint8_t)(x + n) : 0; }
    "#;
    let tmp = std::env::temp_dir().join("cc2_no_string.c");
    std::fs::write(&tmp, src).unwrap();
    let hex_path = std::env::temp_dir().join("cc2_no_string.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            tmp.to_str().unwrap(),
            "-o",
            hex_path.to_str().unwrap(),
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver without string.h: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&hex_path);
}
