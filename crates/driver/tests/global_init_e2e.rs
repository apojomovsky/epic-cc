//! epic-cc#454: a mutable global with an initializer must be written before
//! `main` runs. Before the fix `__start` initialized only const globals, so
//! `static uint8_t x = 0x5A;` read as zero.
//!
//! The fixture reads each global through a separate function, so the value
//! has to survive in RAM rather than being folded into a literal, and both
//! are `volatile` so they stay globals. Simulated result:
//!   x = 0x5A, y = 0x1234  ->  main returns 0x5A + 0x34 = 0x8E

use std::process::Command;

/// A global's RAM address as the *driver* assigned it, read from the
/// `--map` file. Re-deriving the layout in the test would miss ISR-only
/// allocation differences (the map is the authority the asm was built
/// against).
fn mapped_addr(hex_path: &str, global: &str) -> usize {
    let map_path = hex_path.replace(".hex", ".map");
    let text = std::fs::read_to_string(&map_path).expect("read map");
    text.lines()
        .find_map(|l| {
            let l = l.trim();
            l.strip_prefix("global ")
                .and_then(|r| r.split_once(' '))
                .filter(|(name, _)| *name == global)
                .and_then(|(_, addr)| usize::from_str_radix(addr.trim_start_matches("0x"), 16).ok())
        })
        .unwrap_or_else(|| panic!("{global} not in {map_path}"))
}

fn run(device_name: &str) -> pic14_sim::Pic18 {
    let hex_path = format!("tests/fixtures/global_init_{device_name}.hex");
    let map_path = format!("tests/fixtures/global_init_{device_name}.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/global_init.c",
            "-o",
            &hex_path,
            "--map",
            &map_path,
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
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(200_000);
    p
}

#[test]
fn mutable_global_initializers_run_on_pic18() {
    let p = run("18F4550");
    assert!(p.halted(), "program must halt");
    assert_eq!(
        p.ram()[mapped_addr("tests/fixtures/global_init_18F4550.hex", "out")],
        0x8E,
        "x (0x5A) + low byte of y (0x34) must both be initialized; with \
         neither initialized the sum reads 0"
    );
}

/// The same contract on the PIC14 family backend (epic-cc#454's scope note
/// names `isel`, `isel-pic14e` and `isel-pic-baseline` as sharing the
/// `__start` init loop). PIC16F877A runs `isel`.
#[test]
fn mutable_global_initializers_run_on_pic16() {
    let hex_path = "tests/fixtures/global_init_16F877A.hex";
    let map_path = "tests/fixtures/global_init_16F877A.map";
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/global_init.c",
            "-o",
            hex_path,
            "--map",
            map_path,
            "--device",
            "16F877A",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver 16F877A: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(hex_path).expect("read hex");
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(200_000);
    assert!(p.halted(), "program must halt");
    assert_eq!(
        p.ram()[mapped_addr("tests/fixtures/global_init_16F877A.hex", "out")],
        0x8E,
        "x (0x5A) + low byte of y (0x34) must both be initialized on PIC16"
    );
}

/// The ISR path: `__start` is emitted after the ISR body there, by a
/// separate emitter that must materialize pointer-initializer refs too
/// (epic-cc#454 review finding). A NULL `p` reads `out` as 0 instead of 0x5A.
#[test]
fn pointer_global_initializer_survives_an_isr_on_pic16() {
    let hex_path = "tests/fixtures/global_init_isr_16F877A.hex";
    let map_path = "tests/fixtures/global_init_isr_16F877A.map";
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/global_init_isr.c",
            "-o",
            hex_path,
            "--map",
            map_path,
            "--device",
            "16F877A",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver 16F877A isr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(hex_path).expect("read hex");
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(200_000);
    assert!(p.halted(), "program must halt");
    assert_eq!(
        p.ram()[mapped_addr("tests/fixtures/global_init_isr_16F877A.hex", "out")],
        0x5A,
        "p must be initialized to &g so *p reads 0x5A; a NULL p reads 0"
    );
}
