//! Float-preemption acceptance (epic-cc#357): main computes a float chain
//! (fixture `float_isr.c`) and a high-priority ISR fires mid-chain,
//! itself doing float math. Per-context float frames (each context's
//! routine copies place in its own overlay region) keep main's in-flight
//! operands and scratch intact, so main completes with the bit-exact
//! result the un-preempted run produces: 0x42082222 (34.0333328, RNE).
//!
//! The sim fires on a fixed step cadence (gated on GIEH): the float
//! recipes run hundreds of instructions, so of the 8 fires several land
//! INSIDE main's in-flight mul/div. The exact-result assertion is what
//! makes "mid-op" observable (a clobbered operand or scratch perturbs
//! the quotient's last bits).
use std::process::Command;

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

#[test]
fn float_isr_preempts_main_mid_op_without_corruption() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/float_isr.c",
            "-o",
            "tests/fixtures/float_isr.hex",
            "--map",
            "tests/fixtures/float_isr.map",
            "--device",
            "p18f4550",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let hex = std::fs::read_to_string("tests/fixtures/float_isr.hex").unwrap();
    let map = std::fs::read_to_string("tests/fixtures/float_isr.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/float_isr.map");
    let out_addr = map_addr(&map, "out");

    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);

    // Run with the ISR firing on the cadence (mid-chain).
    // Firmware owns INTCON: seed GIEH (bit 7) the way a real program
    // would before enabling its interrupt source. Fire on a fixed step
    // cadence, gated on GIEH (cleared inside the ISR, so no re-entry):
    // the float recipes run hundreds of instructions, so of the 8 fires
    // several land INSIDE main's in-flight mul/div (the exact hazard
    // epic-cc#357 removes).
    // Fires also wait for `main` to be running (steps >= 400 is past
    // __start's clear/init lines): real firmware arms interrupts after
    // the startup zero-clear has finished, so the startup loop must
    // never be preempted. (`fire` cannot gate this: the ISR itself
    // clears it on acknowledge.)
    p.ram_mut()[0xFF2] = 0x80;
    let mut steps = 0usize;
    let mut fired = 0u32;
    while !p.halted() {
        if steps % 250 == 100 && fired < 8 && steps >= 400 && p.ram()[0xFF2] & 0x80 != 0 {
            p.fire_interrupt();
            fired += 1;
        }
        p.step();
        steps += 1;
        if steps > 5_000_000 {
            panic!("program never halted (fired = {fired}, pc = {:#X})", p.pc());
        }
    }
    assert_eq!(fired, 8, "the cadence must deliver all 8 firings");
    // out = 34.0333328 = 0x42082222, LE 22 22 08 42: bit-exact despite the
    // ISR running its own float adds inside main's divide.
    let got = u32::from_le_bytes([
        p.ram()[out_addr],
        p.ram()[out_addr + 1],
        p.ram()[out_addr + 2],
        p.ram()[out_addr + 3],
    ]);
    assert_eq!(got, 0x4208_2222, "main's float chain must be unperturbed");
}
