//! epic-cc#783: an ISR that only adds must narrow its context save (no
//! PROD/TBLPTR/PCLAT saves) while main's in-flight multiply still resumes
//! against its own product. The twin of `prod_isr_e2e`, whose multiplying
//! ISR must keep the PROD save.
use std::process::Command;

/// Globals' RAM addresses, read off the compiler's own `--map` output.
/// Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts (see array_e2e's `map_addr`).
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn compile(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(args)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn run() {
    compile(&[
        "tests/fixtures/isr_narrow.c",
        "-o",
        "tests/fixtures/isr_narrow.hex",
        "--map",
        "tests/fixtures/isr_narrow.map",
        "--device",
        "p18f4550",
    ]);
    let map = std::fs::read_to_string("tests/fixtures/isr_narrow.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/isr_narrow.map");
    let sym = |n: &str| map_addr(&map, n);

    // The narrowed prologue keeps STATUS (the add sets flags) but drops
    // the untouched classes outright.
    compile(&[
        "tests/fixtures/isr_narrow.c",
        "--emit",
        "asm",
        "-o",
        "tests/fixtures/isr_narrow.asm",
        "--device",
        "p18f4550",
    ]);
    let asm = std::fs::read_to_string("tests/fixtures/isr_narrow.asm").expect("read asm");
    let _ = std::fs::remove_file("tests/fixtures/isr_narrow.asm");
    // Scope to the handler: main legitimately moves PRODL into retval
    // after its own multiply, which is not a context save.
    let isr = asm
        .split("isr:")
        .nth(1)
        .expect("ISR body")
        .split("RETFIE")
        .next()
        .expect("ISR epilogue");
    assert!(
        isr.contains("MOVFF 0xFD8,"),
        "STATUS save survives narrowing:\n{isr}"
    );
    for dropped in ["MOVFF 0xFF3,", "MOVFF 0xFF6,", "MOVFF 0xFFA,"] {
        assert!(
            !isr.contains(dropped),
            "untouched save must narrow away ({dropped}):\n{isr}"
        );
    }

    let hex = std::fs::read_to_string("tests/fixtures/isr_narrow.hex").unwrap();
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    // Inputs arrive via the fixture's init stores (epic-cc#561): __start
    // clears zero-initialized globals, so sim-side seeds would not survive.

    // Same window as prod_isr_e2e: PRODL holds main's partial product and
    // main has not yet stored its result. The narrowed handler saves no
    // PROD, so survival here means the live value was never disturbed.
    let main_product = 47u16.wrapping_mul(5) & 0xFF;
    let mut steps = 0;
    while steps < 400 {
        p.step();
        steps += 1;
        if p.ram()[0xFF3] == main_product as u8 && p.ram()[sym("out")] == 0 {
            p.fire_interrupt();
            p.run(4000);
            break;
        }
    }
    assert_eq!(
        p.ram()[sym("out")],
        235,
        "main's product must survive a narrowed ISR (got {})",
        p.ram()[sym("out")],
    );
    assert_eq!(p.ram()[sym("isr_out")], 10, "the ISR's own sum must be 10");
}

#[test]
fn narrowed_isr_preserves_the_preempted_product() {
    run();
}
