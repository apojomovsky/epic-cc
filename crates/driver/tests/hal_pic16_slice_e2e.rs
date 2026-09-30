//! HAL PIC16 slice acceptance (epic-hal#105): the vendored
//! `pic16f87xa-hal` slice compiled through the real `epic-cc` binary
//! (`--target 16F877A`) and run on the `Pic14` simulator. This is the
//! crates/sim e2e epic-hal#67 item (1)/(3) acceptance: a peripheral
//! callback fires end to end (stored by main through the inlined Init,
//! invoked by the ISR via the global, ADR-024) and a const `irq_table`
//! field reads non-zero (the epic-cc#114 zero-blob regression guard).
//!
//! The simulator has no timer hardware, so the test asserts the TMR0IF
//! flag (INTCON bit 2) and fires the interrupt, exactly as the 887 blink
//! smoke and the hal-pic18 slice do. The callback toggles RB0 and bumps
//! `g_toggle_count`, the observable counter the test reads from the
//! address map.

// PIC16F877A SFRs (DS39582B, via the vendored hal_pic16.h).
const INTCON: usize = 0x0B;
const TMR0IF: u8 = 0x04; // INTCON bit 2
const PORTB: usize = 0x06;

/// `in` and `out`'s RAM addresses, read off the compiler's own `--map`
/// output. Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts: the PIC18 path alone parses with
/// switches preserved, and once the frames sit below the globals a
/// difference that far upstream moves every global address.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn fixture(path: &str) -> String {
    format!("tests/fixtures/hal-pic16/{path}")
}

/// Run the real `epic-cc` binary over the given slice sources (with the
/// config TU, exercising CC-3) targeting the 877A. Returns the parsed
/// program words plus the `--map` text so the e2e can locate observable
/// globals by name.
fn compile_slice(sources: &[&str], tag: &str) -> (Vec<u16>, String) {
    let hex_path = std::env::temp_dir().join(format!("hal_pic16_{tag}_{}.hex", std::process::id()));
    let map_path = hex_path.with_extension("map");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F877A", "-I", "tests/fixtures/hal-pic16"])
        .arg("-o")
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg(fixture("hal_pic16_config.c"))
        .args(sources.iter().map(|s| fixture(s)))
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc slice failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read produced hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    (pic14_sim::parse_hex(&produced), map)
}

#[test]
fn callback_blink_toggles_rb0_on_tmr0_overflow() {
    let sources = [
        "hal_pic16_blink.c",
        "hal_pic16_gpio.c",
        "hal_pic16_timer0.c",
        "hal_pic16_irq.c",
        "hal_pic16_dispatch.c",
        "hal_pic16_vector.c",
    ];
    let (prog, map) = compile_slice(&sources, "blink");
    let count_addr = map_addr(&map, "g_toggle_count");
    let seen_addr = map_addr(&map, "g_rb_seen");
    let readback_addr = map_addr(&map, "g_irq_readback");
    let idx_addr = map_addr(&map, "g_irq_idx");

    let mut sim = pic14_sim::Pic14::new(prog);

    // RAM globals are not initialized by the pipeline (the simulator
    // starts zeroed); seed the runtime table index the same way
    // `dynamic_memcpy_e2e` seeds its buffers.
    sim.ram_mut()[idx_addr] = 2;

    // Run past init first (EPIC_TIMER0_Init clears TMR0IF), then assert
    // the irq_table readback landed non-zero (epic-cc#114 shape: a const
    // table field read through a runtime index).
    sim.run(1000);
    assert_ne!(
        sim.ram()[readback_addr],
        0,
        "irq_table field must read non-zero (zero-blob regression)"
    );
    // The runtime index is 2 (TMR0), whose flag mask is INTCON<TMR0IF>.
    assert_eq!(sim.ram()[idx_addr], 2, "g_irq_idx must be 2 (TMR0)");
    assert_eq!(
        sim.ram()[readback_addr] & TMR0IF,
        TMR0IF,
        "TMR0 flag mask must be INTCON bit 2"
    );

    // Fire the Timer0 overflow interrupt three times: the ISR clears
    // TMR0IF and invokes the callback (stored by main through the inlined
    // Init), which toggles RB0 and bumps the counter.
    for _ in 0..3 {
        let before = sim.ram()[count_addr];
        sim.ram_mut()[INTCON] |= TMR0IF; // latch the overflow flag
        sim.fire_interrupt();
        assert_eq!(sim.pc(), 4, "the ISR starts at the vector (word 4)");
        sim.run(20_000);
        assert!(
            sim.ram()[count_addr] > before,
            "callback did not fire (g_toggle_count unchanged)"
        );
    }

    // RB0 toggled 3 times: PORTB bit 0 is set (odd toggles) after 3.
    assert_eq!(
        sim.ram()[PORTB] & 0x01,
        0x01,
        "PORTB bit0 should be set after 3 toggles"
    );
    assert_eq!(sim.ram()[count_addr], 3, "g_toggle_count == 3");
    // The RB change callback never fires in this scenario (no RBIF).
    assert_eq!(sim.ram()[seen_addr], 0, "g_rb_seen must stay 0");
}

#[test]
fn rb_change_callback_fires_with_portb_byte() {
    let sources = [
        "hal_pic16_blink.c",
        "hal_pic16_gpio.c",
        "hal_pic16_timer0.c",
        "hal_pic16_irq.c",
        "hal_pic16_dispatch.c",
        "hal_pic16_vector.c",
    ];
    let (prog, map) = compile_slice(&sources, "rb");
    let seen_addr = map_addr(&map, "g_rb_seen");

    let mut sim = pic14_sim::Pic14::new(prog);

    // Run past init (the RB callback registration), then drive an RB
    // change: set RBIF and a PORTB byte, fire the interrupt, and assert
    // the 1-arg callback received the byte (the param-forwarded shape).
    sim.run(1000);
    sim.ram_mut()[PORTB] = 0xA5;
    sim.ram_mut()[INTCON] |= 0x01; // RBIF
    sim.fire_interrupt();
    sim.run(20_000);
    assert_eq!(
        sim.ram()[seen_addr],
        0xA5,
        "RB callback must have run with the PORTB byte"
    );
}
