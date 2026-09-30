//! Priority-interrupt acceptance (epic-cc#346): a high (`__interrupt(1)`)
//! and a low (`__interrupt(2)`) handler sharing the noinline helper
//! `bump()` with main and each other (fixture `priority_irq.c`), compiled
//! through the whole driver pipeline for the PIC18F4550 and run in the
//! simulator with nested fires.
//!
//! Acceptance: `ticks == 2`, `pkts == 2`, `main_ctr == 3`,
//! `hi_saw_lo == 1`, `mres == 235`, `hres == 42`, `lres == 72`, halted.
//!
//! The injection is deterministic, not pc-pinned: run main until
//! `main_ctr == 1`, fire low, then step until `lo_flag == 1` and fire
//! high. The high fire lands between the low handler's `lo_flag = 1` and
//! `lo_flag = 0` stores by construction (the sim is single-stepped), so
//! `hi_saw_lo == 1` proves the high ISR ran while the low one was live
//! (nesting, not sequential service). The multi-call bodies (`ticks == 2`
//! through two `bump` calls under preemption) prove the three frame
//! regions are disjoint: any clobber would corrupt a counter. The three
//! multiplies (one per context, through per-context `__mul_u8` copies)
//! prove the runtime-routine duplication covers the high copy too.
use std::process::Command;

/// Global RAM addresses (`ticks`, `pkts`, `main_ctr`, results), read off the
/// compiler's own `--map` output. Rebuilding the pipeline here instead
/// would be a second copy of `main.rs` that silently drifts: the PIC18 path
/// alone parses with switches preserved, and once the frames sit below the
/// globals a difference that far upstream moves every global address.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

#[test]
fn priority_interrupts_nest_with_disjoint_frames() {
    let hex_path = "tests/fixtures/priority_irq.hex";
    let map_path = "tests/fixtures/priority_irq.map";

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/priority_irq.c",
            "-o",
            hex_path,
            "--map",
            map_path,
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

    let hex = std::fs::read_to_string(hex_path).unwrap();
    let map = std::fs::read_to_string(map_path).expect("read map");
    let _ = std::fs::remove_file(map_path);
    let addr = |name: &str| map_addr(&map, name);
    let (ticks, pkts, main_ctr, lo_flag, hi_saw_lo) = (
        addr("ticks"),
        addr("pkts"),
        addr("main_ctr"),
        addr("lo_flag"),
        addr("hi_saw_lo"),
    );
    let (mres, hres, lres) = (addr("mres"), addr("hres"), addr("lres"));
    let prog = pic14_sim::parse_hex_pic18(&hex);
    let mut p = pic14_sim::Pic18::new(prog);

    // Run main into its loop body (first `main_ctr` bump landed).
    let mut steps = 0usize;
    while p.ram()[main_ctr] != 1 {
        p.step();
        steps += 1;
        assert!(steps < 10_000, "main never reached its loop");
    }

    // Fire low: the handler starts at the low vector with GIEH still set.
    p.fire_low_interrupt();
    assert_eq!(p.pc(), 0x0018, "low ISR starts at vector 0x0018");

    // Step the low handler to its `lo_flag = 1` store, then fire high:
    // the high ISR runs while the low one is live, by construction.
    steps = 0;
    while p.ram()[lo_flag] != 1 {
        p.step();
        steps += 1;
        assert!(steps < 10_000, "low ISR never raised its flag");
    }
    p.fire_interrupt();
    // Both handlers drain, main completes, `__start` sleeps. Halted comes
    // first: values are only meaningful on a completed run.
    p.run(500_000);
    assert!(p.halted(), "run did not halt (pc = {:#X})", p.pc());
    assert_eq!(p.ram()[ticks], 2, "high body: 0 -> bump -> bump = 2");
    assert_eq!(p.ram()[pkts], 2, "low body survived preemption: 2");
    assert_eq!(p.ram()[main_ctr], 3, "main's state survived both ISRs");
    // Every context multiplied through its own `__mul_u8` copy under
    // preemption: 47*5, 6*7, 8*9.
    assert_eq!(p.ram()[mres], 235, "main's multiply survived both ISRs");
    assert_eq!(p.ram()[hres], 42, "high copy multiplied while low was live");
    assert_eq!(p.ram()[lres], 72, "low copy survived high preemption");
    assert_eq!(
        p.ram()[hi_saw_lo],
        1,
        "high ran while low was live (nesting, not sequential)"
    );
    assert!(p.halted());
}
