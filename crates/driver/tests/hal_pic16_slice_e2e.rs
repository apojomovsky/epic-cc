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

use std::process::Command;

// PIC16F877A SFRs (DS39582B, via the vendored hal_pic16.h).
const INTCON: usize = 0x0B;
const TMR0IF: u8 = 0x04; // INTCON bit 2
const PORTB: usize = 0x06;

fn fixture(path: &str) -> String {
    format!("tests/fixtures/hal-pic16/{path}")
}

/// Run the real `epic-cc` binary over the given slice sources (with the
/// config TU, exercising CC-3) targeting the 877A. Returns the parsed
/// program words for the simulator.
fn compile_slice(sources: &[&str], tag: &str) -> Vec<u16> {
    let hex_path = std::env::temp_dir().join(format!("hal_pic16_{tag}_{}.hex", std::process::id()));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F877A", "-I", "tests/fixtures/hal-pic16"])
        .arg("-o")
        .arg(&hex_path)
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
    let _ = std::fs::remove_file(&hex_path);
    pic14_sim::parse_hex(&produced)
}

/// Rebuild the address map the driver computes for the slice, mirroring
/// `crates/driver/src/main.rs` (the same pattern
/// `hal_pic18_slice_e2e.rs` uses) so the e2e can locate observable
/// globals by name.
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
                    "tests/fixtures/hal-pic16".to_string(),
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
    alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg))
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
    let layout = slice_layout(&sources);
    let count_addr = *layout
        .globals
        .get("g_toggle_count")
        .expect("g_toggle_count") as usize;
    let seen_addr = *layout.globals.get("g_rb_seen").expect("g_rb_seen") as usize;
    let readback_addr = *layout
        .globals
        .get("g_irq_readback")
        .expect("g_irq_readback") as usize;
    let idx_addr = *layout.globals.get("g_irq_idx").expect("g_irq_idx") as usize;

    let prog = compile_slice(&sources, "blink");
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
    let layout = slice_layout(&sources);
    let seen_addr = *layout.globals.get("g_rb_seen").expect("g_rb_seen") as usize;

    let prog = compile_slice(&sources, "rb");
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
