//! epic-cc#622: the narrowed div-rem tail must compute the same digits the
//! wide tail did, through the whole driver pipeline.
//!
//! `bench-u16-dec.c` divides a 16-bit value down by 10 in a loop and stores
//! each remainder's low byte. clang emits the remainder as `mul i16 q, 246`
//! plus a truncating add, and `legalize` narrows that tail to i8 because
//! only the low byte is observed. This runs the real program in the
//! simulator for values that exercise the byte and carry boundaries and
//! checks the five decimal digits.
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
fn the_narrowed_decimal_tail_stores_the_right_digits() {
    let dir = std::env::temp_dir();
    let asm_path = dir.join(format!("u16dec_{}.asm", std::process::id()));
    let hex_path = dir.join(format!("u16dec_{}.hex", std::process::id()));
    for (emit, path) in [(Some("asm"), &asm_path), (None, &hex_path)] {
        let mut args: Vec<String> = vec![
            "tests/fixtures/size-bench/bench-u16-dec.c".into(),
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
    let digits_addr = equ("digits");

    // Values around the byte and carry boundaries, the two widths' edges,
    // and the extremes.
    for v in [0u16, 9, 10, 99, 100, 9999, 256, 25700, 65535] {
        let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
        for _ in 0..start {
            p.step();
        }
        p.ram_mut()[in_addr] = (v & 0xFF) as u8;
        p.ram_mut()[in_addr + 1] = (v >> 8) as u8;
        p.run(200_000);
        assert!(p.halted(), "program must run to completion for {v}");
        let want: Vec<u8> = {
            let mut d = Vec::new();
            let mut x = v;
            for _ in 0..5 {
                d.push((x % 10) as u8);
                x /= 10;
            }
            d
        };
        let got: Vec<u8> = (0..5).map(|i| p.ram()[digits_addr + i]).collect();
        assert_eq!(got, want, "digits of {v}");
    }
}
