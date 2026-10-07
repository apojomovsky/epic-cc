//! epic-cc#767: the in-place 32-bit increment must count exactly like the
//! staged form it replaces. `bench-u32-loop.c` folds its loop-carried
//! `add i32 %i, 1` into the counter's own slot (a 7-word `INCF` chain, no
//! writeback), and the loop compare reads that slot through the fused
//! borrow chain. This runs the real program in the simulator and checks
//! the volatile tick count for limits around the lane-carry boundaries.
//!
//! `limit` is zero-initialized, so seeding happens after `__start` clears
//! RAM: the sim steps past the start window to main's entry before the
//! poke (the same method `u16_decimal_e2e.rs` uses).

use std::process::Command;

/// Steps from reset to main's first instruction: the reset `goto
/// __start`, the clear-loop setup and body, and the `call main` itself.
fn start_steps(asm: &str) -> usize {
    let window = asm
        .split("__start:")
        .nth(1)
        .expect("__start label")
        .split("call main")
        .next()
        .expect("__start must call main");
    let mut steps = 2;
    let mut zero_run = false;
    let mut take: Option<usize> = None;
    for line in window.lines() {
        let line = line.trim();
        if line.starts_with("LFSR") {
            zero_run = true;
            take = None;
            steps += 1;
        } else if let Some(t) = line.strip_prefix("MOVLW 0x") {
            if zero_run {
                take = Some(usize::from(u8::from_str_radix(t, 16).unwrap()));
                steps += 1;
            } else {
                steps += 1;
            }
        } else if line.starts_with("MOVWF") || line.starts_with("MOVLB") {
            steps += 1;
        }
    }
    if let Some(t) = take {
        steps += 3 * t - 1;
    }
    steps
}

#[test]
fn the_folded_u32_counter_counts_like_the_staged_form() {
    let dir = std::env::temp_dir();
    let asm_path = dir.join(format!("u32loop_{}.asm", std::process::id()));
    let hex_path = dir.join(format!("u32loop_{}.hex", std::process::id()));
    for (emit, path) in [(Some("asm"), &asm_path), (None, &hex_path)] {
        let mut args: Vec<String> = vec![
            "tests/fixtures/size-bench/bench-u32-loop.c".into(),
            "--device".into(),
            "18f4550".into(),
            "-o".into(),
            path.display().to_string(),
        ];
        if let Some(f) = emit {
            args.extend(["--emit".into(), f.to_string()]);
        }
        let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
            .args(&args)
            .output()
            .expect("run driver");
        assert!(
            out.status.success(),
            "driver ({emit:?}): {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let asm = std::fs::read_to_string(&asm_path).unwrap();
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let _ = std::fs::remove_file(&asm_path);
    let _ = std::fs::remove_file(&hex_path);
    assert!(
        asm.contains("INCF"),
        "the loop increment must lower in place:\n{asm}"
    );
    let start = start_steps(&asm);
    let equ = |name: &str| -> usize {
        asm.lines()
            .find_map(|l| {
                let l = l.trim();
                let rest = l.strip_prefix(name)?.strip_prefix(" equ ")?;
                usize::from_str_radix(rest.trim_start_matches("0x"), 16).ok()
            })
            .unwrap_or_else(|| panic!("no `{name} equ` in listing"))
    };
    let limit_addr = equ("limit");
    let tick_addr = equ("tick");

    // Each iteration runs `pump` (+1) and `run_once` (+2): three ticks.
    // Limits span zero trips, no carry, the low-lane carry at 256, a wider
    // count, and one trip past 65536 for the next lane's carry; the rest
    // of the 32-bit chain is covered lane by lane in
    // `crates/superopt/tests/add_literal_lane.rs`.
    for limit in [0u32, 1, 2, 5, 85, 255, 256, 257, 300, 66000] {
        let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
        for _ in 0..start {
            p.step();
        }
        for (i, b) in limit.to_le_bytes().iter().enumerate() {
            p.ram_mut()[limit_addr + i] = *b;
        }
        // About forty instructions per trip (two calls plus the increment
        // and compare), so the budget scales with the limit.
        p.run(500 + 100 * limit as usize);
        assert!(p.halted(), "program must run to completion for {limit}");
        let want = limit.wrapping_mul(3) as u8;
        assert_eq!(p.ram()[tick_addr], want, "ticks after {limit} trips");
    }
}
