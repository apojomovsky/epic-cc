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

use std::collections::HashMap;

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
/// config TU, exercising CC-3) targeting the 4550. Returns the program
/// words for the simulator and the global addresses off the compiler's own
/// `--map`. Re-deriving the addresses from a second in-process pipeline
/// would be a copy of `main.rs` that drifts: PIC18 alone parses with
/// switches preserved, and since the frames sit below the globals there, a
/// difference that far upstream moves every global address.
fn compile_slice(sources: &[&str], tag: &str) -> (Vec<u16>, HashMap<String, usize>) {
    let stem = std::env::temp_dir().join(format!("hal_pic18_{tag}_{}", std::process::id()));
    let hex_path = stem.with_extension("hex");
    let map_path = stem.with_extension("map");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "18F4550", "-I", "tests/fixtures/hal-pic18"])
        .arg("-o")
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
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
    let map = std::fs::read_to_string(&map_path).expect("read produced map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    let globals = map
        .lines()
        .filter_map(|l| l.strip_prefix("global "))
        .filter_map(|rest| {
            let (name, addr) = rest.split_once(' ')?;
            let addr = usize::from_str_radix(addr.trim().strip_prefix("0x")?, 16).ok()?;
            Some((name.to_string(), addr))
        })
        .collect();
    (pic14_sim::parse_hex_pic18(&produced), globals)
}

#[test]
fn blink_slice_toggles_rb0_on_tmr0_overflow() {
    let sources = [
        "hal_pic18_blink.c",
        "hal_pic18_gpio.c",
        "hal_pic18_timer0.c",
        "hal_pic18_irq.c",
    ];
    let (prog, globals) = compile_slice(&sources, "blink");
    let count_addr = globals["g_toggle_count"];
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
    let (prog, globals) = compile_slice(&sources, "tick");
    let (e10, e5) = (globals["g_tick_e10"], globals["g_tick_e5"]);
    let result = globals["g_tick_result"];
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
