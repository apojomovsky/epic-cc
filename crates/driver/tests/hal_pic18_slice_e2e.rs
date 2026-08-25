//! HAL PIC18 slice acceptance (epic-cc#75): the vendored
//! `pic16f88x-hal`-analog pic18fxx5x-hal blink and epic-tick programs,
//! compiled through the real `epic-cc` binary (`--target 18F4550`, the
//! config-word TU exercising CC-3) and run on the `Pic18` simulator.
//!
//! The simulator has no timer hardware, so simulated time is driven the
//! same way the 887 blink smoke does: the test asserts the timer flag
//! registers (INTCON<TMR0IF> for the blink, PIR1<TMR2IF> for the tick)
//! and the program's poll loop advances the observable counter. This is
//! the "both programs run correctly on the Pic18 simulator, from a
//! `make test` run" acceptance criterion.

use std::process::Command;

// PIC18F4550 SFRs (DS39632E, via the vendored hal_pic18.h).
const INTCON: usize = 0xFF2;
const PIR1: usize = 0xF9E;
const TMR0IF: u8 = 0x04; // INTCON bit 2
const TMR2IF: u8 = 0x02; // PIR1 bit 1
const LATB: usize = 0xF8A;

fn fixture(path: &str) -> String {
    format!("tests/fixtures/hal-pic18/{path}")
}

/// Run the real `epic-cc` binary over the given slice sources (with the
/// config TU, exercising CC-3) targeting the 4550. Returns the parsed
/// program words for the simulator.
fn compile_slice(sources: &[&str], tag: &str) -> Vec<u16> {
    let hex_path = std::env::temp_dir().join(format!("hal_pic18_{tag}_{}.hex", std::process::id()));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "18F4550", "-I", "tests/fixtures/hal-pic18"])
        .arg("-o")
        .arg(&hex_path)
        .arg(fixture("hal_pic18_config.c"))
        .args(sources.iter().map(|s| fixture(s)))
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc slice failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read produced hex");
    let _ = std::fs::remove_file(&hex_path);
    pic14_sim::parse_hex_pic18(&produced)
}

/// Rebuild the address map the driver computes for the slice, mirroring
/// `crates/driver/src/main.rs`: every .c input through clang + llvm-link,
/// then the whole-program stages, so the e2e can locate observable globals
/// by name (the same pattern `multi_tu_e2e.rs` and `cc2_headers_e2e.rs`
/// use).
/// The driver ships the freestanding headers (`stdint.h`, `epic-cc.h`,
/// ...) in a temp include dir; the layout helper needs them too so clang
/// parses the slice exactly as the real driver does.
fn header_dir() -> std::path::PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tid = format!("{:?}", std::thread::current().id());
    let dir = std::env::temp_dir()
        .join(format!(
            "hal-slice-test-{}-{}-{}",
            std::process::id(),
            tid,
            ns
        ))
        .join("include");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("stdint.h"), driver::stdint_h::STDINT_H).unwrap();
    std::fs::write(dir.join("stdbool.h"), driver::stdbool_h::STDBOOL_H).unwrap();
    std::fs::write(dir.join("stddef.h"), driver::stddef_h::STDDEF_H).unwrap();
    std::fs::write(dir.join("string.h"), driver::string_h::STRING_H).unwrap();
    std::fs::write(dir.join("epic-cc.h"), driver::epic_cc_h::EPIC_CC_H).unwrap();
    dir
}

fn slice_layout(sources: &[&str]) -> alloc::AllocLayout {
    use driver::clang_discovery;
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let llvm_link = clang_discovery::resolve_llvm_link(&clang).expect("resolve_llvm_link");
    let hdir = header_dir();

    let tid = format!("{:?}", std::thread::current().id());
    let tmp = std::env::temp_dir().join(format!("epiccc-hal-slice-{}-{}", std::process::id(), tid));
    std::fs::create_dir_all(&tmp).unwrap();

    let mut units = Vec::new();
    for (n, input) in sources.iter().enumerate() {
        let ll_path = tmp.join(format!("{n:03}.ll"));
        driver::clang::compile_to_file(
            &clang,
            &resdir,
            std::path::Path::new(&fixture(input)),
            &ll_path,
            &driver::clang::Options {
                includes: vec![
                    "tests/fixtures/hal-pic18".to_string(),
                    hdir.to_str().unwrap().to_string(),
                ],
                header_dir: Some(hdir.clone()),
                ..Default::default()
            },
        );
        units.push(ll_path);
    }

    let merged_path = tmp.join("merged.ll");
    let mut cmd = Command::new(&llvm_link);
    cmd.arg("-S");
    for u in &units {
        cmd.arg(u);
    }
    cmd.args(["-o", merged_path.to_str().unwrap()]);
    let out = cmd.output().expect("run llvm-link");
    assert!(
        out.status.success(),
        "llvm-link: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let ll_text =
        irparse::sanitize_symbols(&std::fs::read_to_string(&merged_path).expect("read merged .ll"));
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    alloc::allocate(&device::PIC18F4550, &m, &callgraph::edges_text(&cg))
}

#[test]
fn blink_slice_toggles_rb0_on_tmr0_overflow() {
    let sources = [
        "hal_pic18_blink.c",
        "hal_pic18_gpio.c",
        "hal_pic18_timer0.c",
        "hal_pic18_irq.c",
    ];
    let layout = slice_layout(&sources);
    let count_addr = *layout
        .globals
        .get("g_toggle_count")
        .expect("g_toggle_count") as usize;

    let prog = compile_slice(&sources, "blink");
    let mut sim = pic14_sim::Pic18::new(prog);

    // Run past init first (EPIC_TIMER0_Init clears TMR0IF), then drive
    // three Timer0 overflows by asserting TMR0IF each step until the
    // blink loop clears it and toggles RB0.
    sim.run(100);
    for _ in 0..3 {
        let before = sim.ram()[count_addr];
        let mut steps = 0;
        while sim.ram()[count_addr] == before && steps < 20_000 {
            sim.ram_mut()[INTCON] |= TMR0IF; // keep the flag asserted
            sim.step();
            steps += 1;
        }
        assert!(
            sim.ram()[count_addr] > before,
            "blink did not consume TMR0IF (steps={steps})"
        );
    }

    // RB0 toggled 3 times: LATB bit 0 is set (odd toggles) after 3.
    assert_eq!(
        sim.ram()[LATB] & 0x01,
        0x01,
        "LATB bit0 should be set after 3 toggles"
    );
    assert_eq!(sim.ram()[count_addr], 3, "g_toggle_count == 3");
}

#[test]
fn tick_slice_advances_1ms_per_tmr2_overflow() {
    let sources = [
        "hal_pic18_tick_demo.c",
        "hal_pic18_tick.c",
        "hal_pic18_timer2.c",
        "hal_pic18_irq.c",
    ];
    let layout = slice_layout(&sources);
    let e10 = *layout.globals.get("g_tick_e10").expect("g_tick_e10") as usize;
    let e5 = *layout.globals.get("g_tick_e5").expect("g_tick_e5") as usize;
    let result = *layout.globals.get("g_tick_result").expect("g_tick_result") as usize;

    let prog = compile_slice(&sources, "tick");
    let mut sim = pic14_sim::Pic18::new(prog);

    // Run to just past init (epic_tick_init clears TMR2IF), then pump the
    // sim with TMR2IF asserted every step: the delay loop consumes it and
    // advances the tick once per flag. Run until the result global is
    // written (both delays done).
    sim.run(100);
    let mut steps = 0;
    while sim.ram()[result] == 0 && steps < 200_000 {
        sim.ram_mut()[PIR1] |= TMR2IF; // a 1 ms tick every instruction
        sim.step();
        steps += 1;
    }
    assert_eq!(sim.ram()[e10], 10, "e10 (delay 10 ms) should be exactly 10");
    assert_eq!(sim.ram()[e5], 5, "e5 (delay 5 ms after 10) should be 5");
    assert_eq!(sim.ram()[result], 1, "tick ok flag should be 1 (PASS)");
}
