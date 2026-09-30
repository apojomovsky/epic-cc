//! epic-cc#493 regression: an ISR that seeds the FSRs must not corrupt the
//! preempted main context's in-flight copy pointer.
//!
//! main's `g_storage = *h` lowers to a POSTINC walk: FSR1 is seeded with
//! h's frame slot and advanced byte by byte, so the pointer is live
//! across the copy's instructions. The fixture's handler copies its own
//! struct through an FSR0/FSR1 loop, and epic-cc#477's FSR save/restore
//! in every ISR prologue and epilogue is what lets the preempted walk
//! resume. An interrupt taken inside main's window and served without
//! that restore would resume main against the ISR's pointer.
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

/// Sorted RAM addresses of one function's frame locals (`local
/// {func}::{name} 0xNN` lines of the same `--map` output).
fn frame_addrs(map: &str, fname: &str) -> Vec<u16> {
    let prefix = format!("local {fname}::");
    let mut addrs: Vec<u16> = map
        .lines()
        .filter(|l| l.starts_with(&prefix))
        .map(|l| {
            let addr = l.rsplit(' ').next().expect("map local has an address");
            u16::from_str_radix(addr.trim_start_matches("0x"), 16).expect("map address is hex")
        })
        .collect();
    addrs.sort();
    addrs
}

fn run() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/fsr1_isr.c",
            "-o",
            "tests/fixtures/fsr1_isr.hex",
            "--map",
            "tests/fixtures/fsr1_isr.map",
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
    let asm_out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/fsr1_isr.c",
            "-o",
            "tests/fixtures/fsr1_isr.asm",
            "--device",
            "p18f4550",
            "--emit",
            "asm",
        ])
        .output()
        .expect("run driver for asm");
    assert!(
        asm_out.status.success(),
        "driver asm: {}",
        String::from_utf8_lossy(&asm_out.stderr)
    );
    let asm = std::fs::read_to_string("tests/fixtures/fsr1_isr.asm").unwrap();
    let _ = std::fs::remove_file("tests/fixtures/fsr1_isr.asm");
    let map = std::fs::read_to_string("tests/fixtures/fsr1_isr.map").expect("read map");
    let _ = std::fs::remove_file("tests/fixtures/fsr1_isr.map");
    let sym = |n: &str| map_addr(&map, n);
    // The handler must actually seed FSR1, or this test proves nothing.
    assert!(
        asm.contains("0xFE1"),
        "the ISR must seed/save FSR1 for this fixture to exercise the window:\n{asm}"
    );

    let hex = std::fs::read_to_string("tests/fixtures/fsr1_isr.hex").unwrap();
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);
    // The ISR's source struct carries a distinguishable pattern.
    for (i, b) in [0xAAu8, 0xBB, 0xCC, 0xDD].iter().enumerate() {
        p.ram_mut()[sym("isr_src") + i] = *b;
    }

    // Fire only while main's copy is in flight: FSR1L must hold the
    // address h's slot points at (params sit at the frame base, so the
    // lowest `store_handle::` map address; main stores &h there). The
    // value occurs only at the copy's seed step, and the nonzero guard
    // skips the zeroed reset state.
    let h_slot = frame_addrs(&map, "store_handle")
        .into_iter()
        .min()
        .expect("store_handle has a frame slot");
    let g_out = sym("g_out");
    let mut steps = 0;
    let mut fired = false;
    while steps < 4000 {
        p.step();
        steps += 1;
        let f = p.ram()[0xFE1];
        if f != 0 && f == p.ram()[h_slot as usize] && p.ram()[g_out] == 0 {
            p.fire_interrupt();
            p.run(20_000);
            fired = true;
            break;
        }
    }
    assert!(
        fired,
        "never observed main mid-copy to inject the interrupt"
    );

    assert_eq!(
        p.ram()[sym("g_out")],
        0x55,
        "main's copy must survive an ISR that seeds FSR1 (g_out should be cb_impl's 0x55)"
    );
}

#[test]
fn isr_copy_preserves_the_preempted_fsr1_copy_pointer() {
    run();
}
