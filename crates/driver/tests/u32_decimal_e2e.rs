//! epic-cc#722: the shared `__udec_u32` digit helper must emit the same
//! digits the three expanded per-site loops did, through the whole driver
//! pipeline.
//!
//! `u32-decimal.c` divides a 32-bit value down by 10 in a value-driven loop
//! and stores each ASCII digit into a global buffer. `legalize_pic18`
//! shares the loop as one `__udec_u32` call; the test asserts the call is
//! present (so the test cannot pass on the unshared lowering) and runs the
//! real program in the simulator for values that exercise the byte, width
//! and carry boundaries, checking every digit plus the count.
//!
//! `in` is a zero-initialized global, so `__start` clears it before main.
//! Seeding must therefore happen after the clear: the asm listing gives the
//! `__start` window length, and the sim is advanced past it to main's entry
//! before the poke (the same method `crates/isel-pic18/tests` uses).

use std::process::Command;

/// Words the PIC18 program runs before `main`'s first instruction: the
/// reset `goto __start`, the clear-loop setup, the loop body, and the
/// `call main` itself. The body is 3 instructions per cleared byte minus
/// the last iteration's skipped `BRA`, and the simulator's PC is on main's
/// entry once the call has been fetched.
fn start_steps(asm: &str) -> usize {
    let window = asm
        .split("__start:")
        .nth(1)
        .expect("__start label")
        .split("call main")
        .next()
        .expect("__start must call main");
    let mut steps = 2; // the reset `goto __start`, and the `call main`
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
        // CLRF and DECFSZ run `t` times; BRA runs `t - 1` times.
        steps += 3 * t - 1;
    }
    steps
}

#[test]
fn the_shared_digit_helper_stores_the_right_digits() {
    let dir = std::env::temp_dir();
    let asm_path = dir.join(format!("u32dec_{}.asm", std::process::id()));
    let hex_path = dir.join(format!("u32dec_{}.hex", std::process::id()));
    for (emit, path) in [(Some("asm"), &asm_path), (None, &hex_path)] {
        let mut args: Vec<String> = vec![
            "tests/fixtures/u32-decimal.c".into(),
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
        asm.contains("CALL __udec_u32"),
        "fixture must route through the helper"
    );
    let start = start_steps(&asm);

    // The emitted HEX is laid out by the driver's full pipeline, which is
    // not the in-process one above (banking and peephole run after alloc),
    // so read the addresses the HEX actually used from the listing's `equ`
    // lines rather than trusting a separately-computed layout.
    let equ = |name: &str| -> usize {
        asm.lines()
            .find_map(|l| {
                let l = l.trim();
                let rest = l.strip_prefix(name)?.strip_prefix(" equ ")?;
                usize::from_str_radix(rest.trim_start_matches("0x"), 16).ok()
            })
            .unwrap_or_else(|| panic!("no `{name} equ` in listing"))
    };
    let in_addr = equ("in");
    let buf_addr = equ("buf");
    let buf2_addr = equ("buf2");
    let n_addr = equ("n");

    // Values around the byte and carry boundaries, the 16-bit edge, powers
    // of 10, and the 32-bit extremes.
    for v in [
        0u32, 9, 10, 99, 100, 256, 65535, 65536, 1234567890, 999999999, 1000000000, 4294967295,
    ] {
        let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
        for _ in 0..start {
            p.step();
        }
        for i in 0..4 {
            p.ram_mut()[in_addr + i] = ((v >> (8 * i)) & 0xFF) as u8;
        }
        p.run(200_000);
        assert!(p.halted(), "program must run to completion for {v}");
        let want: Vec<u8> = {
            let mut d = Vec::new();
            let mut x = v;
            loop {
                d.push(b'0' + (x % 10) as u8);
                x /= 10;
                if x == 0 {
                    break;
                }
            }
            d
        };
        let got: Vec<u8> = (0..want.len()).map(|i| p.ram()[buf_addr + i]).collect();
        assert_eq!(got, want, "digits of {v}");
        let got2: Vec<u8> = (0..want.len()).map(|i| p.ram()[buf2_addr + i]).collect();
        assert_eq!(got2, want, "second digits of {v}");
        assert_eq!(p.ram()[n_addr] as usize, want.len(), "count of {v}");
    }
}
