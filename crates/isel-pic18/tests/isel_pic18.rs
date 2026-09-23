use device::PIC18F4550;
use ir::parse;
use isel_pic18::select;
use std::collections::HashMap;

fn addrs(pairs: &[(&str, u16)]) -> HashMap<String, u16> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

/// Set the initializer bytes of a const global. The canonical IR text
/// (`ir::parse`) carries no bytes: those arrive from irparse's `.ll`
/// decode, so tests that exercise the emitted `db` table / `TBLRD` path
/// must supply them the way irparse would.
fn with_bytes(mut m: ir::Module, name: &str, bytes: &[u8]) -> ir::Module {
    for g in &mut m.globals {
        if g.name == name {
            g.bytes = bytes.to_vec();
            g.size = bytes.len() as u16;
        }
    }
    m
}

/// Set the ref entries of a const global the way irparse records a
/// `ptr @target` field: one entry per byte of the pointer, at absolute
/// blob offsets. The canonical IR text carries no refs.
fn with_refs(mut m: ir::Module, name: &str, refs: &[(usize, &str)]) -> ir::Module {
    for g in &mut m.globals {
        if g.name == name {
            g.refs = refs.iter().map(|(o, f)| (*o, f.to_string())).collect();
        }
    }
    m
}

/// A hand-built ISR module needs an epic-cc#477 save area for PROD/FSR1
/// (the fixed block has no room); any disjoint address works here.
fn select_isr(device: &device::Device, m: &ir::Module, addrs: &HashMap<String, u16>) -> String {
    isel_pic18::select_with_locs(device, m, addrs, None, Some(0x0040), None).0
}

/// Count the instructions `select` emits between the `__start:` label and
/// `call main` (epic-cc#561): per zero-run of `take` bytes, one LFSR and
/// one MOVLW plus a body of `3*take - 1` (CLRF and DECFSZ `take` times,
/// BRA `take - 1` times: the last DECFSZ skips); per initializer byte,
/// MOVLW and MOVWF, plus an MOVLB when the target is banked.
fn start_steps(asm: &str) -> usize {
    let window = asm
        .split("__start:")
        .nth(1)
        .expect("__start label")
        .split("    call main")
        .next()
        .expect("__start must call main");
    let mut steps = 1; // the call itself
    let mut zero_run = false; // the MOVLW after an LFSR carries the take
    for line in window.lines() {
        let line = line.trim();
        if line.starts_with("LFSR") {
            zero_run = true;
            steps += 1;
        } else if let Some(take) = line.strip_prefix("MOVLW 0x") {
            if zero_run {
                // the MOVLW itself plus the clear-loop body
                steps += 3 * usize::from(u8::from_str_radix(take, 16).unwrap());
                zero_run = false;
            } else {
                steps += 1;
            }
        } else if line.starts_with("MOVWF") || line.starts_with("MOVLB") {
            steps += 1;
        }
    }
    steps
}

/// Advance a fresh simulator through `__start`'s clear/init window
/// (`start_steps` instructions), leaving it parked at main's entry: the
/// clear would otherwise erase a `ram_mut` poke made before `run`.
/// Seeding from here is sim-observable behavior identical to a debugger
/// poking RAM after reset-init.
fn step_past_start(p: &mut pic14_sim::Pic18, steps: usize) {
    assert!(!p.halted());
    for _ in 0..steps {
        p.step();
    }
    assert!(!p.halted(), "__start window must not halt the machine");
}

#[test]
fn empty_function_emits_a_bare_return() {
    let m = parse("fn main(void) ()\n  block entry:\n    ret void\n");
    let asm = select(&PIC18F4550, &m, &addrs(&[]), None);
    assert!(asm.contains("RETURN"), "asm:\n{asm}");
}

#[test]
fn i32_add_emits_a_four_byte_carry_chain() {
    let m = parse(
        "global a i32\nglobal b i32\nglobal out i32\n\
         fn main(void) ()\n  block entry:\n    %1 = load i32 @a\n    %2 = load i32 @b\n\
         %3 = add i32 %1, %2\n    store i32 %3 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x24),
        ("out", 0x28),
        ("main::1", 0x30),
        ("main::2", 0x34),
        ("main::3", 0x38),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("ADDWF 0x030,W,A"), "byte 0 add:\n{asm}");
    assert_eq!(
        asm.matches("ADDWFC").count(),
        3,
        "bytes 1-3 carry adds:\n{asm}"
    );
    assert_eq!(asm.matches("MOVWF").count(), 4, "four result bytes:\n{asm}");
}

#[test]
fn i32_icmp_eq_ne_compare_all_four_bytes() {
    let m = parse(
        "global a i32\nglobal b i32\nglobal o1 i8\nglobal o2 i8\n\
         fn main(void) ()\n  block entry:\n\
         %1 = load i32 @a\n    %2 = load i32 @b\n\
         %3 = icmp eq i32 %1, %2\n    store i8 %3 @o1\n\
         %4 = icmp ne i32 %1, %2\n    store i8 %4 @o2\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x24),
        ("o1", 0x28),
        ("o2", 0x29),
        ("main::1", 0x30),
        ("main::2", 0x34),
        ("main::3", 0x38),
        ("main::4", 0x39),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("SUBWF").count(),
        8,
        "four bytes x two predicates:\n{asm}"
    );
    assert_eq!(asm.matches("BNZ").count(), 8, "mismatch branches:\n{asm}");
}

#[test]
fn i32_icmp_ugt_compares_high_byte_first() {
    let m = parse(
        "global a i32\nglobal b i32\nglobal o1 i8\nfn main(void) ()\n  block entry:\n\
         %1 = load i32 @a\n    %2 = load i32 @b\n\
         %3 = icmp ugt i32 %1, %2\n    store i8 %3 @o1\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x24),
        ("o1", 0x28),
        ("main::1", 0x30),
        ("main::2", 0x34),
        ("main::3", 0x38),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("SUBWF 0x033,W,A"),
        "high byte (offset 3) compared first:\n{asm}"
    );
}

#[test]
fn const_shl_i16_emits_rlcf_chain() {
    let m = parse(
        "global a i16\nglobal out i16\nfn main(void) ()\n  block entry:\n\
         %1 = load i16 @a\n    %2 = shl i16 %1, 3\n    store i16 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x24),
        ("main::1", 0x26),
        ("main::2", 0x28),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(asm.matches("RLCF").count(), 6, "3 shifts x 2 bytes:\n{asm}");
    assert_eq!(
        asm.matches("BCF 0xFD8,0,A").count(),
        3,
        "clear carry before each shift step:\n{asm}"
    );
}

/// Compiles `out = a << k` (i16) through the real selector, assembles, and
/// simulates every 16-bit input against the shift's arithmetic meaning,
/// with a per-case entry W so a construction that depends on stale W
/// fails loudly instead of only on curated inputs. `dst` is the shift's
/// temp base: access-bank layouts hide bank-selection bugs, so straddle
/// layouts must go through here too.
fn assert_shl16_domain_exact(k: i64, dst: u16) {
    let m = parse(&format!(
        "global a i16\nglobal out i16\nfn main(void) ()\n  block entry:\n\
         %1 = load i16 @a\n    %2 = shl i16 %1, {k}\n    store i16 %2 @out\n    ret void\n"
    ));
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x24),
        ("main::1", 0x26),
        ("main::2", dst),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let start = start_steps(&asm);
    for x in 0..=u16::MAX {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start);
        p.ram_mut()[0x20] = x as u8;
        p.ram_mut()[0x21] = (x >> 8) as u8;
        p.set_w((x as u8) ^ ((x >> 8) as u8));
        p.run(200);
        let got = u16::from(p.ram()[0x24]) | (u16::from(p.ram()[0x25]) << 8);
        assert_eq!(got, x << k, "a={x:#06x} << {k} (dst={dst:#06x})");
        assert!(p.halted(), "program must run to completion (a={x:#06x})");
    }
}

#[test]
fn const_shl_i16_constructions_are_exact_for_amounts_4_to_7() {
    for k in [4, 5, 6, 7] {
        assert_shl16_domain_exact(k, 0x28);
    }
}

#[test]
fn const_shl_i16_constructions_survive_bank_straddle_layouts() {
    // dst at 0x05F crosses the access-bank edge, 0x0FF and 0x1FF put the
    // two lanes in different banks: every construction op must address
    // its lane through a freshly fetched operand, not a cached bank
    // letter, or the far lane silently lands 0x100 bytes away.
    for dst in [0x05F, 0x0FF, 0x1FF] {
        for k in [4, 5, 6, 7] {
            assert_shl16_domain_exact(k, dst);
        }
    }
}

#[test]
fn const_shl_i16_by_4_drops_the_per_bit_unroll() {
    let m = parse(
        "global a i16\nglobal out i16\nfn main(void) ()\n  block entry:\n\
         %1 = load i16 @a\n    %2 = shl i16 %1, 4\n    store i16 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x24),
        ("main::1", 0x26),
        ("main::2", 0x28),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(asm.matches("RLCF").count(), 0, "no unroll steps:\n{asm}");
    assert_eq!(asm.matches("SWAPF").count(), 3, "nibble swap x3:\n{asm}");
    assert_eq!(asm.matches("ANDWF").count(), 2, "two masks:\n{asm}");
    assert_eq!(
        asm.matches("IORWF").count(),
        1,
        "one straddled-nibble recombine:\n{asm}"
    );
    assert_eq!(
        asm.matches("BCF 0xFD8,0,A").count(),
        0,
        "no carry clears:\n{asm}"
    );
}

#[test]
fn const_shl_i16_by_7_becomes_a_right_shift_and_byte_move() {
    let m = parse(
        "global a i16\nglobal out i16\nfn main(void) ()\n  block entry:\n\
         %1 = load i16 @a\n    %2 = shl i16 %1, 7\n    store i16 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x24),
        ("main::1", 0x26),
        ("main::2", 0x28),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(asm.matches("RLCF").count(), 0, "no unroll steps:\n{asm}");
    assert_eq!(
        asm.matches("RRCF").count(),
        3,
        "x >> 1 twice through C, then place the last bit:\n{asm}"
    );
}

#[test]
fn const_shl_i16_by_2_keeps_the_unrolled_form() {
    // Amounts 2 and 3 have no verified shortcut: superopt found nothing
    // within a tractable search bound (docs/41). Pin that no special case
    // silently fires for them.
    let m = parse(
        "global a i16\nglobal out i16\nfn main(void) ()\n  block entry:\n\
         %1 = load i16 @a\n    %2 = shl i16 %1, 2\n    store i16 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x24),
        ("main::1", 0x26),
        ("main::2", 0x28),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(asm.matches("RLCF").count(), 4, "2 steps x 2 bytes:\n{asm}");
    assert_eq!(
        asm.matches("BCF 0xFD8,0,A").count(),
        2,
        "clear carry before each shift step:\n{asm}"
    );
}

#[test]
fn const_shl_i32_by_4_keeps_the_unrolled_form() {
    // The 16-bit constructions do not extend to 4 lanes without their own
    // superopt verification (docs/41 defers this); the unroll must stay.
    let m = parse(
        "global a i32\nglobal out i32\nfn main(void) ()\n  block entry:\n\
         %1 = load i32 @a\n    %2 = shl i32 %1, 4\n    store i32 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x28),
        ("main::1", 0x30),
        ("main::2", 0x34),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(asm.matches("RLCF").count(), 16, "4 steps x 4 bytes:\n{asm}");
    assert_eq!(
        asm.matches("BCF 0xFD8,0,A").count(),
        4,
        "clear carry before each shift step:\n{asm}"
    );
}

#[test]
fn const_lshr_i16_by_4_uses_the_verified_nibble_form() {
    // dst at 0x0FF puts the second lane in the next bank, exercising the
    // banked operand text the construction emits for each lane.
    for dst in [0x28, 0x0FF] {
        let m = parse(
            "global a i16\nglobal out i16\nfn main(void) ()\n  block entry:\n\
             %1 = load i16 @a\n    %2 = lshr i16 %1, 4\n    store i16 %2 @out\n    ret void\n",
        );
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x24),
            ("main::1", 0x26),
            ("main::2", dst),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert_eq!(asm.matches("RRCF").count(), 0, "no per-bit unroll:\n{asm}");
        assert_eq!(
            asm.matches("SWAPF").count(),
            3,
            "three nibble swaps:\n{asm}"
        );
        assert_eq!(
            asm.matches("BCF 0xFD8,0,A").count(),
            0,
            "no carry clears:\n{asm}"
        );
        // The opcode counts above are the shape; this is the contract: the
        // emitted asm, simulated over every 16-bit input, must compute the
        // shift. Entry W is set to a value the construction must not depend
        // on (it clobbers W), same as the i8 test.
        let words = asm::assemble_pic18(&asm);
        for x in 0..=u16::MAX {
            let mut p = pic14_sim::Pic18::new(words.clone());
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = x as u8;
            p.ram_mut()[0x21] = (x >> 8) as u8;
            p.set_w((x as u8) ^ ((x >> 8) as u8));
            p.run(200);
            let got = u16::from(p.ram()[0x24]) | (u16::from(p.ram()[0x25]) << 8);
            assert_eq!(got, x >> 4, "a={x:#06x} >> 4 (dst={dst:#06x})");
            assert!(p.halted(), "program must run to completion (a={x:#06x})");
        }
    }
}

#[test]
fn const_shl_i8_by_4_is_a_swapf_mask_and_matches_across_the_byte() {
    // dst at 0x1FF checks the banked form of the same trick; the access
    // layout additionally pins the exact access-mode operand text.
    for dst in [0x23, 0x1FF] {
        let m = parse(
            "global a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
             %1 = load i8 @a\n    %2 = shl i8 %1, 4\n    store i8 %2 @out\n    ret void\n",
        );
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x21),
            ("main::1", 0x22),
            ("main::2", dst),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert_eq!(asm.matches("RLCF").count(), 0, "no unroll steps:\n{asm}");
        if dst == 0x23 {
            assert_eq!(
                asm.matches("SWAPF 0x023,W,A").count(),
                1,
                "swap through W, mask, store:\n{asm}"
            );
        }
        let words = asm::assemble_pic18(&asm);
        for b in 0..=u8::MAX {
            let mut p = pic14_sim::Pic18::new(words.clone());
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = b;
            p.set_w(!b);
            p.run(200);
            assert_eq!(p.ram()[0x21], b << 4, "a={b:#04x} << 4 (dst={dst:#06x})");
            assert!(p.halted(), "program must run to completion (a={b:#04x})");
        }
    }
}

#[test]
fn const_lshr_i16_by_12_shifts_only_the_surviving_lane() {
    // k = 12 -> m = 1 byte move plus r = 4 residual over the single
    // surviving lane at dst[0] (right shifts keep dst[0..n-m)). Distinct
    // code path from the two-lane amount-4 form: active == 1 with m > 0.
    for dst in [0x28, 0x0FF] {
        let m = parse(
            "global a i16\nglobal out i16\nfn main(void) ()\n  block entry:\n\
             %1 = load i16 @a\n    %2 = lshr i16 %1, 12\n    store i16 %2 @out\n    ret void\n",
        );
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x24),
            ("main::1", 0x26),
            ("main::2", dst),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert_eq!(asm.matches("RRCF").count(), 0, "no unroll steps:\n{asm}");
        let words = asm::assemble_pic18(&asm);
        for x in 0..=u16::MAX {
            let mut p = pic14_sim::Pic18::new(words.clone());
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = x as u8;
            p.ram_mut()[0x21] = (x >> 8) as u8;
            p.set_w((x as u8) ^ ((x >> 8) as u8));
            p.run(200);
            let got = u16::from(p.ram()[0x24]) | (u16::from(p.ram()[0x25]) << 8);
            assert_eq!(got, x >> 12, "a={x:#06x} >> 12 (dst={dst:#06x})");
            assert!(p.halted(), "program must run to completion (a={x:#06x})");
        }
    }
}

#[test]
fn const_lshr_i8_by_4_is_a_swapf_mask_and_matches_across_the_byte() {
    // Mirror of const_shl_i8_by_4: the single-lane right shift by 4 keeps
    // the low nibble of the nibble-swapped byte, 3 words against the
    // 8-word unroll.
    for dst in [0x21, 0x1FF] {
        let m = parse(
            "global a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
             %1 = load i8 @a\n    %2 = lshr i8 %1, 4\n    store i8 %2 @out\n    ret void\n",
        );
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x21),
            ("main::1", 0x22),
            ("main::2", dst),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert_eq!(asm.matches("RRCF").count(), 0, "no unroll steps:\n{asm}");
        assert_eq!(asm.matches("SWAPF").count(), 1, "swap through W:\n{asm}");
        let words = asm::assemble_pic18(&asm);
        for b in 0..=u8::MAX {
            let mut p = pic14_sim::Pic18::new(words.clone());
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = b;
            p.set_w(!b);
            p.run(200);
            assert_eq!(p.ram()[0x21], b >> 4, "a={b:#04x} >> 4 (dst={dst:#06x})");
            assert!(p.halted(), "program must run to completion (a={b:#04x})");
        }
    }
}

#[test]
fn const_shl_i8_by_5_6_7_extend_the_nibble_form() {
    // epic-cc#573: the single-lane nibble form plus one BCF-seeded RLCF
    // per extra bit, 5/7/9 words against the 10/12/14-word unroll. Same
    // two layouts as the amount-4 test, simulated over the byte domain
    // with hostile entry W (the construction clobbers W).
    for r in [5, 6, 7] {
        for dst in [0x23, 0x1FF] {
            let m = parse(&format!(
                "global a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
                 %1 = load i8 @a\n    %2 = shl i8 %1, {r}\n    store i8 %2 @out\n    ret void\n",
            ));
            let addrs = addrs(&[
                ("a", 0x20),
                ("out", 0x21),
                ("main::1", 0x22),
                ("main::2", dst),
            ]);
            let asm = select(&PIC18F4550, &m, &addrs, None);
            assert_eq!(asm.matches("SWAPF").count(), 1, "one nibble swap:\n{asm}");
            assert_eq!(
                asm.matches("RLCF").count(),
                (r - 4) as usize,
                "one rotate per extra bit:\n{asm}"
            );
            assert_eq!(
                asm.matches("BCF 0xFD8,0,A").count(),
                (r - 4) as usize,
                "one carry seed per extra bit:\n{asm}"
            );
            let words = asm::assemble_pic18(&asm);
            for b in 0..=u8::MAX {
                let mut p = pic14_sim::Pic18::new(words.clone());
                // Park past __start's zero-clear (epic-cc#561) before the
                // poke: the clear would otherwise erase it.
                step_past_start(&mut p, start_steps(&asm));
                p.ram_mut()[0x20] = b;
                p.set_w(!b);
                p.run(200);
                assert_eq!(
                    p.ram()[0x21],
                    b.wrapping_shl(r as u32),
                    "a={b:#04x} << {r} (dst={dst:#06x})"
                );
                assert!(p.halted(), "program must run to completion (a={b:#04x})");
            }
        }
    }
}

#[test]
fn const_lshr_i8_by_5_6_7_extend_the_nibble_form() {
    // Mirror of the left test: nibble down-shift plus one BCF-seeded
    // RRCF per extra bit. Covers the EPIC_IRQ_GetFlag situs (>>5/>>6
    // on one byte) at i8 directly.
    for r in [5, 6, 7] {
        for dst in [0x21, 0x1FF] {
            let m = parse(&format!(
                "global a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
                 %1 = load i8 @a\n    %2 = lshr i8 %1, {r}\n    store i8 %2 @out\n    ret void\n",
            ));
            let addrs = addrs(&[
                ("a", 0x20),
                ("out", 0x21),
                ("main::1", 0x22),
                ("main::2", dst),
            ]);
            let asm = select(&PIC18F4550, &m, &addrs, None);
            assert_eq!(asm.matches("SWAPF").count(), 1, "one nibble swap:\n{asm}");
            assert_eq!(
                asm.matches("RRCF").count(),
                (r - 4) as usize,
                "one rotate per extra bit:\n{asm}"
            );
            assert_eq!(
                asm.matches("BCF 0xFD8,0,A").count(),
                (r - 4) as usize,
                "one carry seed per extra bit:\n{asm}"
            );
            let words = asm::assemble_pic18(&asm);
            for b in 0..=u8::MAX {
                let mut p = pic14_sim::Pic18::new(words.clone());
                // Park past __start's zero-clear (epic-cc#561) before the
                // poke: the clear would otherwise erase it.
                step_past_start(&mut p, start_steps(&asm));
                p.ram_mut()[0x20] = b;
                p.set_w(!b);
                p.run(200);
                assert_eq!(
                    p.ram()[0x21],
                    b.wrapping_shr(r as u32),
                    "a={b:#04x} >> {r} (dst={dst:#06x})"
                );
                assert!(p.halted(), "program must run to completion (a={b:#04x})");
            }
        }
    }
}

#[test]
fn const_lshr_i32_by_28_rotates_only_the_surviving_lane() {
    // Mirror of const_shl_i32_by_28: k = 28 -> m = 3 byte moves plus r = 4
    // residual. For a right shift the surviving lane is dst[0] (not the top
    // lane the left form uses), and the residual is a nibble down-shift.
    // Domain is the same 256 derived byte patterns the left test uses;
    // dst at 0x0FF puts the lane in the next bank.
    for dst in [0x2C, 0x0FF] {
        let m = parse(
            "global a i32\nglobal out i32\nfn main(void) ()\n  block entry:\n\
             %1 = load i32 @a\n    %2 = lshr i32 %1, 28\n    store i32 %2 @out\n    ret void\n",
        );
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x24),
            ("main::1", 0x40),
            ("main::2", dst),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert_eq!(asm.matches("RRCF").count(), 0, "no unroll steps:\n{asm}");
        let words = asm::assemble_pic18(&asm);
        for b in 0..=u8::MAX {
            let mut p = pic14_sim::Pic18::new(words.clone());
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = b;
            p.ram_mut()[0x21] = b ^ 0x5A;
            p.ram_mut()[0x22] = !b;
            p.ram_mut()[0x23] = b.rotate_left(3);
            p.set_w(b);
            p.run(300);
            let got = u32::from(p.ram()[0x24])
                | u32::from(p.ram()[0x25]) << 8
                | u32::from(p.ram()[0x26]) << 16
                | u32::from(p.ram()[0x27]) << 24;
            let x = u32::from(b)
                | u32::from(b ^ 0x5A) << 8
                | u32::from(!b) << 16
                | u32::from(b.rotate_left(3)) << 24;
            assert_eq!(got, x >> 28, "a={x:#010x} >> 28 (dst={dst:#06x})");
            assert!(p.halted(), "program must run to completion (a={b:#04x})");
        }
    }
}

#[test]
fn const_shl_i16_by_12_rotates_only_the_surviving_lane() {
    // k = 12 splits into a one-byte move plus a 4-bit residual over the
    // single surviving lane: the SWAPF trick applies there, not the
    // 16-bit construction, and no RLCF step may remain. dst at 0x0FF
    // puts that lane in the next bank.
    for dst in [0x28, 0x0FF] {
        let m = parse(
            "global a i16\nglobal out i16\nfn main(void) ()\n  block entry:\n\
             %1 = load i16 @a\n    %2 = shl i16 %1, 12\n    store i16 %2 @out\n    ret void\n",
        );
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x24),
            ("main::1", 0x26),
            ("main::2", dst),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert_eq!(asm.matches("RLCF").count(), 0, "no unroll steps:\n{asm}");
        let words = asm::assemble_pic18(&asm);
        for x in 0..=u16::MAX {
            let mut p = pic14_sim::Pic18::new(words.clone());
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = x as u8;
            p.ram_mut()[0x21] = (x >> 8) as u8;
            p.set_w((x as u8) & ((x >> 8) as u8));
            p.run(200);
            let got = u16::from(p.ram()[0x24]) | (u16::from(p.ram()[0x25]) << 8);
            assert_eq!(got, x << 12, "a={x:#06x} << 12 (dst={dst:#06x})");
        }
    }
}

#[test]
fn const_shl_i32_by_7_uses_the_fused_form() {
    // m = 0, r = 7 over four lanes: the fused family fires (21 words against
    // the 35-word unroll). dst at 0x0FD puts the top lane (dst+3 = 0x100) in
    // a fresh bank, so the banked operand text is exercised too.
    for dst in [0x30, 0x0FD] {
        let m = parse(
            "global a i32\nglobal out i32\nfn main(void) ()\n  block entry:\n\
             %1 = load i32 @a\n    %2 = shl i32 %1, 7\n    store i32 %2 @out\n    ret void\n",
        );
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x24),
            ("main::1", 0x40),
            ("main::2", dst),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert_eq!(asm.matches("RLCF").count(), 0, "no unroll steps:\n{asm}");
        assert_eq!(asm.matches("RRNCF").count(), 4, "one rotate pass:\n{asm}");
        let words = asm::assemble_pic18(&asm);
        for b in 0..=u8::MAX {
            let mut p = pic14_sim::Pic18::new(words.clone());
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = b;
            p.ram_mut()[0x21] = b ^ 0x5A;
            p.ram_mut()[0x22] = !b;
            p.ram_mut()[0x23] = b.rotate_left(3);
            p.set_w(b.wrapping_add(0x99));
            p.run(400);
            let got = u32::from(p.ram()[0x24])
                | u32::from(p.ram()[0x25]) << 8
                | u32::from(p.ram()[0x26]) << 16
                | u32::from(p.ram()[0x27]) << 24;
            let x = u32::from(b)
                | u32::from(b ^ 0x5A) << 8
                | u32::from(!b) << 16
                | u32::from(b.rotate_left(3)) << 24;
            assert_eq!(got, x << 7, "a={x:#010x} << 7 (dst={dst:#06x})");
            assert!(p.halted(), "program must run to completion (a={b:#04x})");
        }
    }
}

#[test]
fn const_shl_i32_by_6_uses_the_fused_form() {
    // r = 6 over four lanes: two rotate passes (8 words) plus the combine.
    for dst in [0x30, 0x0FD] {
        let m = parse(
            "global a i32\nglobal out i32\nfn main(void) ()\n  block entry:\n\
             %1 = load i32 @a\n    %2 = shl i32 %1, 6\n    store i32 %2 @out\n    ret void\n",
        );
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x24),
            ("main::1", 0x40),
            ("main::2", dst),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert_eq!(asm.matches("RLCF").count(), 0, "no unroll steps:\n{asm}");
        assert_eq!(asm.matches("RRNCF").count(), 8, "two rotate passes:\n{asm}");
        let words = asm::assemble_pic18(&asm);
        for b in 0..=u8::MAX {
            let mut p = pic14_sim::Pic18::new(words.clone());
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = b;
            p.ram_mut()[0x21] = b ^ 0x5A;
            p.ram_mut()[0x22] = !b;
            p.ram_mut()[0x23] = b.rotate_left(3);
            p.set_w(b.wrapping_add(0x99));
            p.run(400);
            let got = u32::from(p.ram()[0x24])
                | u32::from(p.ram()[0x25]) << 8
                | u32::from(p.ram()[0x26]) << 16
                | u32::from(p.ram()[0x27]) << 24;
            let x = u32::from(b)
                | u32::from(b ^ 0x5A) << 8
                | u32::from(!b) << 16
                | u32::from(b.rotate_left(3)) << 24;
            assert_eq!(got, x << 6, "a={x:#010x} << 6 (dst={dst:#06x})");
            assert!(p.halted(), "program must run to completion (a={b:#04x})");
        }
    }
}

#[test]
fn const_shl_i32_by_28_rotates_only_the_surviving_lane() {
    // m = 3 leaves one live lane, so the SWAPF trick fires there too.
    // dst at 0x0FD puts that lane (dst+3 = 0x100) in a fresh bank.
    for dst in [0x30, 0x0FD] {
        let m = parse(
            "global a i32\nglobal out i32\nfn main(void) ()\n  block entry:\n\
             %1 = load i32 @a\n    %2 = shl i32 %1, 28\n    store i32 %2 @out\n    ret void\n",
        );
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x24),
            ("main::1", 0x40),
            ("main::2", dst),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert_eq!(asm.matches("RLCF").count(), 0, "no unroll steps:\n{asm}");
        let words = asm::assemble_pic18(&asm);
        for b in 0..=u8::MAX {
            let mut p = pic14_sim::Pic18::new(words.clone());
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = b;
            p.ram_mut()[0x21] = b ^ 0x5A;
            p.ram_mut()[0x22] = !b;
            p.ram_mut()[0x23] = b.rotate_left(3);
            p.set_w(b);
            p.run(300);
            let got = u32::from(p.ram()[0x24])
                | u32::from(p.ram()[0x25]) << 8
                | u32::from(p.ram()[0x26]) << 16
                | u32::from(p.ram()[0x27]) << 24;
            let x = u32::from(b)
                | u32::from(b ^ 0x5A) << 8
                | u32::from(!b) << 16
                | u32::from(b.rotate_left(3)) << 24;
            assert_eq!(got, x << 28, "a={x:#010x} << 28 (dst={dst:#06x})");
            assert!(p.halted(), "program must run to completion (a={b:#04x})");
        }
    }
}

#[test]
fn const_shl_i32_by_30_rotates_only_the_surviving_lane() {
    // epic-cc#573: k = 30 -> m = 3 byte moves plus r = 6 residual over the
    // single surviving lane (nibble plus two seeded rotates, 7 words
    // against the 12-word unroll). Same derived patterns as the by-28
    // test; dst at 0x0FD puts the lane in a fresh bank.
    for dst in [0x30, 0x0FD] {
        let m = parse(
            "global a i32\nglobal out i32\nfn main(void) ()\n  block entry:\n\
             %1 = load i32 @a\n    %2 = shl i32 %1, 30\n    store i32 %2 @out\n    ret void\n",
        );
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x24),
            ("main::1", 0x40),
            ("main::2", dst),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert_eq!(asm.matches("SWAPF").count(), 1, "one nibble swap:\n{asm}");
        assert_eq!(
            asm.matches("RLCF").count(),
            2,
            "two residual rotates:\n{asm}"
        );
        let words = asm::assemble_pic18(&asm);
        for b in 0..=u8::MAX {
            let mut p = pic14_sim::Pic18::new(words.clone());
            // Park past __start's zero-clear (epic-cc#561) before the
            // poke: the clear would otherwise erase it.
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = b;
            p.ram_mut()[0x21] = b ^ 0x5A;
            p.ram_mut()[0x22] = !b;
            p.ram_mut()[0x23] = b.rotate_left(3);
            p.set_w(b);
            p.run(300);
            let got = u32::from(p.ram()[0x24])
                | u32::from(p.ram()[0x25]) << 8
                | u32::from(p.ram()[0x26]) << 16
                | u32::from(p.ram()[0x27]) << 24;
            let x = u32::from(b)
                | u32::from(b ^ 0x5A) << 8
                | u32::from(!b) << 16
                | u32::from(b.rotate_left(3)) << 24;
            assert_eq!(got, x << 30, "a={x:#010x} << 30 (dst={dst:#06x})");
            assert!(p.halted(), "program must run to completion (a={b:#04x})");
        }
    }
}

#[test]
fn const_lshr_i32_by_30_rotates_only_the_surviving_lane() {
    // Mirror of the left test: k = 30 -> m = 3 plus r = 6 over dst[0].
    for dst in [0x30, 0x0FD] {
        let m = parse(
            "global a i32\nglobal out i32\nfn main(void) ()\n  block entry:\n\
             %1 = load i32 @a\n    %2 = lshr i32 %1, 30\n    store i32 %2 @out\n    ret void\n",
        );
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x24),
            ("main::1", 0x40),
            ("main::2", dst),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert_eq!(asm.matches("SWAPF").count(), 1, "one nibble swap:\n{asm}");
        assert_eq!(
            asm.matches("RRCF").count(),
            2,
            "two residual rotates:\n{asm}"
        );
        let words = asm::assemble_pic18(&asm);
        for b in 0..=u8::MAX {
            let mut p = pic14_sim::Pic18::new(words.clone());
            // Park past __start's zero-clear (epic-cc#561) before the
            // poke: the clear would otherwise erase it.
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = b;
            p.ram_mut()[0x21] = b ^ 0x5A;
            p.ram_mut()[0x22] = !b;
            p.ram_mut()[0x23] = b.rotate_left(3);
            p.set_w(b);
            p.run(300);
            let got = u32::from(p.ram()[0x24])
                | u32::from(p.ram()[0x25]) << 8
                | u32::from(p.ram()[0x26]) << 16
                | u32::from(p.ram()[0x27]) << 24;
            let x = u32::from(b)
                | u32::from(b ^ 0x5A) << 8
                | u32::from(!b) << 16
                | u32::from(b.rotate_left(3)) << 24;
            assert_eq!(got, x >> 30, "a={x:#010x} >> 30 (dst={dst:#06x})");
            assert!(p.halted(), "program must run to completion (a={b:#04x})");
        }
    }
}

#[test]
fn const_ashr_i32_sign_fills() {
    let m = parse(
        "global a i32\nglobal out i32\nfn main(void) ()\n  block entry:\n\
         %1 = load i32 @a\n    %2 = ashr i32 %1, 4\n    store i32 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x24),
        ("main::1", 0x28),
        ("main::2", 0x2C),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("RRCF").count(),
        16,
        "4 shifts x 4 bytes:\n{asm}"
    );
    assert!(
        asm.contains("BTFSC"),
        "sign-bit test before each asr step:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "poison")]
fn const_shift_count_out_of_range_panics() {
    let m = parse("global x i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @x\n    %2 = shl i8 %1, 8\n    ret void\n");
    let _ = select(
        &PIC18F4550,
        &m,
        &addrs(&[("x", 0x20), ("main::1", 0x21), ("main::2", 0x22)]),
        None,
    );
}

#[test]
fn load_and_store_i8_use_movff() {
    let m = parse("global in i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    store i8 %1 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x10), ("out", 0x11), ("main::1", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVFF 0x010, 0x012"),
        "load into %1's slot:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0x012, 0x011"),
        "store %1 to out:\n{asm}"
    );
}

// clang's own -O1 GlobalOpt narrows an internal flag only ever written 0/1
// down to `global i1` (epic-cc#462), so i1 reaches isel as a memory type and
// must lower exactly like the one-byte i8 path.
#[test]
fn load_and_store_i1_use_the_byte_path() {
    let m = parse("global in i1\nglobal out i1\nfn main(void) ()\n  block entry:\n    %1 = load i1 @in\n    store i1 %1 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x10), ("out", 0x11), ("main::1", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVFF 0x010, 0x012"),
        "one-byte load into %1's slot:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0x012, 0x011"),
        "one-byte store to out:\n{asm}"
    );
}

#[test]
fn store_an_i1_constant_writes_the_byte_value() {
    let m = parse(
        "global out i1\nfn main(void) ()\n  block entry:\n    store i1 1 @out\n    ret void\n",
    );
    let addrs = addrs(&[("out", 0x11)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("MOVLW 0x01"), "asm:\n{asm}");
    assert!(asm.contains("MOVWF 0x011,A"), "asm:\n{asm}");
}

// The SFR literal-pointer branch MOVFF-copies the raw byte (no masking),
// so pin it for i1 too: the address needs no bank select on the access
// bank, and the one-byte width is the whole store.
#[test]
fn store_an_i1_through_a_literal_pointer_writes_the_sfr() {
    let m = parse("fn main(void) ()\n  block entry:\n    store i1 1 0xF81\n    ret void\n");
    let asm = select(&PIC18F4550, &m, &addrs(&[]), None);
    assert!(asm.contains("MOVLW 0x01"), "asm:\n{asm}");
    assert!(
        asm.contains("MOVWF 0x081,A"),
        "SFR store must be a=0, no MOVLB:\n{asm}"
    );
}

/// Build a module with a runtime routine (name, param widths, __scr size)
/// plus a main that calls it, so the recipe-emission path is exercised
/// through `select` exactly as legalize's injected modules reach it.
#[test]
fn mul_u8_recipe_uses_hardware_mulwf() {
    // The P6 headline: the u8 mul is ONE MULWF (W x f -> PRODH:PRODL), no
    // shift-add loop.
    let m = parse(
        "fn __mul_u8(i8) (a=i8, b=i8)\n  block entry:\n    %__scr = alloca 6\n    ret i8 0\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    let addrs = addrs(&[
        ("__mul_u8::a", 0x20),
        ("__mul_u8::b", 0x21),
        ("__mul_u8::__scr", 0x30),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MULWF"),
        "the u8 mul must use hardware MULWF:\n{asm}"
    );
    assert!(!asm.contains("RLF"), "no shift-add loop on PIC18:\n{asm}");
}

#[test]
fn runtime_u16_mul_uses_schoolbook_partials() {
    let m = parse(
        "fn __mul_u16(i16) (a=i16, b=i16)\n  block entry:\n    %__scr = alloca 14\n    ret i16 0\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    let addrs = addrs(&[
        ("__mul_u16::a", 0x20),
        ("__mul_u16::b", 0x22),
        ("__mul_u16::__scr", 0x30),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    // P00 (shift 0), P01 + P10 (shift 8) contribute to the low 16 bits;
    // P11 (shift 16) is dropped. So exactly 3 hardware MULWF partials.
    assert_eq!(
        asm.matches("MULWF").count(),
        3,
        "schoolbook partials, P11 dropped:\n{asm}"
    );
}

#[test]
fn udiv_u16_recipe_emits_restoring_loop() {
    let m = parse(
        "fn __udiv_u16(i16) (num=i16, den=i16)\n  block entry:\n    %__scr = alloca 7\n    ret i16 0\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    let addrs = addrs(&[
        ("__udiv_u16::num", 0x20),
        ("__udiv_u16::den", 0x22),
        ("__udiv_u16::__scr", 0x30),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("RLCF").count(),
        4,
        "16 iterations, num+rem shift:\n{asm}"
    );
    assert_eq!(asm.matches("DECFSZ").count(), 1, "loop counter:\n{asm}");
}

#[test]
fn load_and_store_i16_copy_both_bytes_low_then_high() {
    let m = parse("global in i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @in\n    store i16 %1 @out\n    ret void\n");
    let addrs = addrs(&[("in", 0x10), ("out", 0x12), ("main::1", 0x14)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("MOVFF 0x010, 0x014"));
    assert!(asm.contains("MOVFF 0x011, 0x015"));
    assert!(asm.contains("MOVFF 0x014, 0x012"));
    assert!(asm.contains("MOVFF 0x015, 0x013"));
}

#[test]
fn store_a_constant_uses_movlw_then_movwf() {
    // MOVFF has no literal-source form — a constant must go through W.
    let m = parse(
        "global out i8\nfn main(void) ()\n  block entry:\n    store i8 5 @out\n    ret void\n",
    );
    let addrs = addrs(&[("out", 0x11)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("MOVLW 0x05"));
    assert!(
        asm.contains("MOVWF 0x011,A") || asm.contains("MOVWF 0x11,A"),
        "asm:\n{asm}"
    );
}

#[test]
fn literal_ptr_store_writes_the_sfr_with_no_bank() {
    // PORTB = 0xF81 on the PIC18F4550 (SFR segment, a=0, no MOVLB).
    let m = parse("fn main(void) ()\n  block entry:\n    store i8 85 0xF81\n    ret void\n");
    let asm = select(&PIC18F4550, &m, &addrs(&[]), None);
    assert!(
        asm.contains("MOVWF 0x081,A"),
        "SFR store must be a=0, no MOVLB:\n{asm}"
    );
    assert!(
        !asm.contains("MOVLB"),
        "SFR access must not touch BSR:\n{asm}"
    );
}

#[test]
fn literal_ptr_load_copies_from_the_sfr() {
    let m = parse("global out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 0xF81\n    store i8 %1 @out\n    ret void\n");
    let addrs = addrs(&[("out", 0x10), ("main::1", 0x11)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVFF 0xF81, 0x011"),
        "SFR load must copy from 0xF81:\n{asm}"
    );
}

#[test]
fn literal_ptr_reg_store_copies_via_movff() {
    let m = parse("global in i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @in\n    store i8 %1 0xF81\n    ret void\n");
    let addrs = addrs(&[("in", 0x10), ("main::1", 0x11)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVFF 0x011, 0xF81"),
        "SFR reg store must copy via MOVFF:\n{asm}"
    );
}

#[test]
fn isr_emits_vector_prologue_and_retfie() {
    let m = parse(
        "fn isr(void) [isr] ()\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    let asm = select_isr(&PIC18F4550, &m, &addrs(&[]));
    assert!(
        asm.contains("org 0x0008"),
        "ISR must be placed at the high vector:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0x000, 0x00C"),
        "retval snapshot lo:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0x003, 0x00F"),
        "retval snapshot hi:\n{asm}"
    );
    assert!(asm.contains("MOVFF 0xFD8, 0x009"), "STATUS save:\n{asm}");
    assert!(asm.contains("MOVFF 0xFE0, 0x00A"), "BSR save:\n{asm}");
    assert!(asm.contains("MOVFF 0xFE9, 0x00B"), "FSR0L save:\n{asm}");
    assert!(asm.contains("MOVFF 0xFEA, 0x004"), "FSR0H save:\n{asm}");
    assert!(asm.contains("MOVFF 0xFF6, 0x005"), "TBLPTRL save:\n{asm}");
    assert!(asm.contains("MOVFF 0xFF7, 0x006"), "TBLPTRH save:\n{asm}");
    assert!(asm.contains("MOVFF 0xFF8, 0x007"), "TBLPTRU save:\n{asm}");
    assert!(asm.contains("MOVWF 0x008,A"), "W save last:\n{asm}");
    // The epilogue: reverse order, MOVFF-based (flags survive), W last via
    // MOVF (the one accepted flag clobber), then RETFIE.
    assert!(
        asm.contains("MOVFF 0x00F, 0x003"),
        "retval restore hi:\n{asm}"
    );
    assert!(asm.contains("MOVFF 0x009, 0xFD8"), "STATUS restore:\n{asm}");
    assert!(asm.contains("MOVF 0x008, W, A"), "W restore:\n{asm}");
    assert!(asm.contains("RETFIE"), "ISR must end with RETFIE:\n{asm}");
}

#[test]
fn non_isr_functions_still_emit_plain_return() {
    let m = parse("fn main(void) ()\n  block entry:\n    ret void\n");
    let asm = select(&PIC18F4550, &m, &addrs(&[]), None);
    assert!(asm.contains("RETURN"));
    assert!(!asm.contains("RETFIE"));
    assert!(!asm.contains("org 0x0008"));
}

#[test]
#[should_panic(expected = "at most one high- and one low-priority")]
fn two_isrs_panic_loudly() {
    let m = parse(
        "fn isr1(void) [isr] ()\n  block entry:\n    ret void\n\
         fn isr2(void) [isr] ()\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    let _ = select_isr(&PIC18F4550, &m, &addrs(&[]));
}

#[test]
#[should_panic(expected = "must be void")]
fn isr_returning_a_value_panics() {
    let m = parse(
        "fn isr(i8) [isr] ()\n  block entry:\n    ret i8 5\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    let _ = select_isr(&PIC18F4550, &m, &addrs(&[]));
}

#[test]
fn i8_binops_load_b_into_w_then_operate_against_a() {
    let cases: &[(&str, &str)] = &[
        ("add", "ADDWF"),
        ("sub", "SUBWF"),
        ("and", "ANDWF"),
        ("or", "IORWF"),
        ("xor", "XORWF"),
    ];
    for (op, mne) in cases {
        let m = parse(&format!(
            "global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = {op} i8 %1, %2\n    ret void\n"
        ));
        let addrs = addrs(&[
            ("a", 0x10),
            ("b", 0x11),
            ("main::1", 0x12),
            ("main::2", 0x13),
            ("main::3", 0x14),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert!(
            asm.contains(&format!("{mne} 0x012,W,A")) || asm.contains(&format!("{mne} 0x12,W,A")),
            "{op}:\n{asm}"
        );
    }
}

#[test]
fn i8_binop_dest_at_banked_address_routes_through_operand_with_bank_suffix() {
    // The destination slot (main::3) lands at 0x180 (bank 1, f=0x80) — an
    // address >= 0x60 that requires an explicit access-bank suffix (and a
    // MOVLB) — this is the banked-destination case the brief's first
    // example (all addrs < 0x60) would not have caught.
    let m = parse(
        "global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = add i8 %1, %2\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x10),
        ("b", 0x11),
        ("main::1", 0x12),
        ("main::2", 0x13),
        ("main::3", 0x180),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVWF 0x80,B") || asm.contains("MOVWF 0x080,B"),
        "banked dest must go through operand() with an explicit ,B suffix:\n{asm}"
    );
    assert!(
        asm.contains("MOVLB 0x1"),
        "banked dest must emit a MOVLB for bank 1:\n{asm}"
    );
}

#[test]
fn i16_add_uses_addwfc_for_the_high_byte() {
    let m = parse("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = add i16 %1, %2\n    ret void\n");
    let addrs = addrs(&[
        ("a", 0x10),
        ("b", 0x12),
        ("main::1", 0x14),
        ("main::2", 0x16),
        ("main::3", 0x18),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("ADDWF") && asm.contains("ADDWFC"),
        "low byte plain add, high byte with carry:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    // seed a=0x00FF, b=0x0001 -> 0x0100, exercises the carry chain.
    p.ram_mut()[0x10] = 0xFF;
    p.ram_mut()[0x11] = 0x00;
    p.ram_mut()[0x12] = 0x01;
    p.ram_mut()[0x13] = 0x00;
    p.run(200);
    assert_eq!(p.ram()[0x18], 0x00);
    assert_eq!(p.ram()[0x19], 0x01);
}

#[test]
fn i16_sub_uses_subfwb_for_the_high_byte() {
    let m = parse("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = sub i16 %1, %2\n    ret void\n");
    let addrs = addrs(&[
        ("a", 0x10),
        ("b", 0x12),
        ("main::1", 0x14),
        ("main::2", 0x16),
        ("main::3", 0x18),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    // 0x0100 - 0x0001 = 0x00FF, exercises the borrow chain.
    p.ram_mut()[0x10] = 0x00;
    p.ram_mut()[0x11] = 0x01;
    p.ram_mut()[0x12] = 0x01;
    p.ram_mut()[0x13] = 0x00;
    p.run(200);
    assert_eq!(p.ram()[0x18], 0xFF);
    assert_eq!(p.ram()[0x19], 0x00);
}

#[test]
fn i16_bitwise_ops_apply_independently_per_byte() {
    for (op, mne) in [("and", "ANDWF"), ("or", "IORWF"), ("xor", "XORWF")] {
        let m = parse(&format!(
            "global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = {op} i16 %1, %2\n    ret void\n"
        ));
        let addrs = addrs(&[
            ("a", 0x10),
            ("b", 0x12),
            ("main::1", 0x14),
            ("main::2", 0x16),
            ("main::3", 0x18),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        // Both bytes use the same plain (non-carry) mnemonic, applied twice.
        assert_eq!(asm.matches(mne).count(), 2, "{op}:\n{asm}");
    }
}

#[test]
fn i8_binop_const_lhs_sub_emits_sublw() {
    // `sub i8 5, %x` (`k - a`) is now handled via `SUBLW` for byte 0
    // (and the `k - a` chain for `n > 1`), not rejected. `add i8 5, %x`
    // is handled as commutative via operand swap. Verify `sub` emits
    // `SUBLW 0x05` and runs correctly.
    let m = parse(
        "global x i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @x\n    %2 = sub i8 5, %1\n    ret void\n",
    );
    let addrs = addrs(&[("x", 0x10), ("main::1", 0x12), ("main::2", 0x13)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("SUBLW 0x05"),
        "sub i8 5, %x should emit SUBLW 0x05, got:\n{asm}"
    );
}

#[test]
fn wide_const_lhs_sub_skipped_lane_flags_match_unskipped() {
    // epic-cc#575 review: with the saved bit clear the surviving flags
    // come from `COMF` rather than the skipped `ADDLW 0x00`, so `C`/`Z`
    // coincidence is sim-asserted here, differentially: the unskipped
    // listing is the skipped one with the dead `ADDLW` put back, and
    // both must leave the same `C`/`Z` for saved-set and saved-clear
    // inputs (`x` low byte below/above `0x12` decides the saved bit).
    let m = parse(
        "global a i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = sub i16 18, %1\n    store i16 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x24),
        ("main::1", 0x26),
        ("main::2", 0x28),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("ADDLW 0x00"),
        "skipped listing must have no ADDLW 0x00:\n{asm}"
    );
    let mut unskipped = String::new();
    let mut restored = 0;
    for line in asm.lines() {
        unskipped.push_str(line);
        unskipped.push('\n');
        if line.trim_start().starts_with("COMF ") {
            unskipped.push_str("    ADDLW 0x00\n");
            restored += 1;
        }
    }
    assert_eq!(restored, 1, "exactly one zero lane to restore:\n{asm}");
    let skipped_words = asm::assemble_pic18(&asm);
    let unskipped_words = asm::assemble_pic18(&unskipped);
    for x in [0x0012u16, 0x0013, 0x0000, 0x00FF, 0xFFFF] {
        let run = |words: &[u16]| {
            let mut p = pic14_sim::Pic18::new(words.to_vec());
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = x as u8;
            p.ram_mut()[0x21] = (x >> 8) as u8;
            p.set_w((x as u8) ^ ((x >> 8) as u8));
            p.run(200);
            assert!(p.halted(), "program must run to completion ({x:#06x})");
            let out = u16::from(p.ram()[0x24]) | (u16::from(p.ram()[0x25]) << 8);
            (out, p.ram()[0xFD8])
        };
        let (got_skip, st_skip) = run(&skipped_words);
        let (got_unskip, st_unskip) = run(&unskipped_words);
        assert_eq!(got_skip, 0x0012u16.wrapping_sub(x), "result ({x:#06x})");
        assert_eq!(got_skip, got_unskip, "same result ({x:#06x})");
        assert_eq!(
            st_skip & 0x01,
            st_unskip & 0x01,
            "C must coincide ({x:#06x})"
        );
        assert_eq!(
            st_skip & 0x04,
            st_unskip & 0x04,
            "Z must coincide ({x:#06x})"
        );
    }
}

#[test]
fn wide_const_lhs_sub_zero_lane_skips_addlw() {
    // epic-cc#575: `sub i16 0x0012, %x` has a zero high lane whose
    // `ADDLW 0x00` adds nothing to `~a`. The fold drops it and stays
    // exact over the full 16-bit domain, with a garbage entry W so a
    // construction that depends on stale W fails loudly.
    let m = parse(
        "global a i16\nglobal out i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = sub i16 18, %1\n    store i16 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x24),
        ("main::1", 0x26),
        ("main::2", 0x28),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("ADDLW 0x00"),
        "zero high lane must skip its ADDLW:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for x in 0..=u16::MAX {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x20] = x as u8;
        p.ram_mut()[0x21] = (x >> 8) as u8;
        p.set_w((x as u8) ^ ((x >> 8) as u8));
        p.run(200);
        let got = u16::from(p.ram()[0x24]) | (u16::from(p.ram()[0x25]) << 8);
        assert_eq!(got, 0x0012u16.wrapping_sub(x), "0x0012 - {x:#06x}");
        assert!(p.halted(), "program must run to completion ({x:#06x})");
    }
}

#[test]
fn wide_const_lhs_sub_i32_zero_lanes_skip_addlw() {
    // epic-cc#575: the i32 version (`0x12000012 - a`) drops both zero
    // lanes. The full domain is too large to simulate, so the corners
    // that stress the borrow chain (lane boundaries, all-ones, k
    // itself) stand in for it.
    let m = parse(
        "global a i32\nglobal out i32\nfn main(void) ()\n  block entry:\n    %1 = load i32 @a\n    %2 = sub i32 301989906, %1\n    store i32 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x24),
        ("main::1", 0x28),
        ("main::2", 0x2C),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("ADDLW 0x00").count(),
        0,
        "both zero lanes must skip their ADDLW:\n{asm}"
    );
    assert!(
        asm.contains("ADDLW 0x12"),
        "nonzero lanes keep their ADDLW:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for x in [
        0x00000000u32,
        0x00000001,
        0x00000012,
        0x00000013,
        0x000000FF,
        0x00000100,
        0x0000FFFF,
        0x00010000,
        0x0011FFFF,
        0x12000011,
        0x12000012,
        0x12000013,
        0xEDCBA987,
        0xFFFFFFFF,
    ] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        for (i, b) in x.to_le_bytes().iter().enumerate() {
            p.ram_mut()[0x20 + i] = *b;
        }
        p.set_w((x as u8) ^ ((x >> 24) as u8));
        p.run(400);
        let got = u32::from_le_bytes([p.ram()[0x24], p.ram()[0x25], p.ram()[0x26], p.ram()[0x27]]);
        assert_eq!(got, 0x12000012u32.wrapping_sub(x), "0x12000012 - {x:#010x}");
        assert!(p.halted(), "program must run to completion ({x:#010x})");
    }
}

#[test]
#[should_panic(expected = "variable-count")]
fn i8_binop_const_lhs_is_rejected_not_silently_miscompiled() {
    // `shl i8 1, %x` has a const LHS (`b.a = 1`) and a variable count
    // (`b.b = %x`). Const-count shifts are inlined from `b.b`; a
    // variable count must be a routine call (legalize). Reaching isel
    // with it means a missing legalize rewrite, so isel must panic rather
    // than silently miscompile `1 << %x` as `%x << 1`.
    let m = parse(
        "global x i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @x\n    %2 = shl i8 1, %1\n    ret void\n",
    );
    let addrs = addrs(&[("x", 0x10), ("main::1", 0x12), ("main::2", 0x13)]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
#[should_panic(expected = "const-LHS")]
fn shift_byte_granular_const_lhs_is_rejected_not_silently_miscompiled() {
    // `shl i32 7, 8` takes the m > 0 byte-move arm, which reads the
    // operand straight out of RAM via `val_addr` -- and `val_addr` maps
    // `Val::Const(k)` to the truncated RAM ADDRESS `k & 0xFF`. Without
    // the guard the MOVFF chain would move whatever bytes live at 0x007
    // instead of the literal 7. This must fail loudly instead.
    let m = parse(
        "global out i32\nfn main(void) ()\n  block entry:\n    %1 = shl i32 7, 8\n    store i32 %1 @out\n    ret void\n",
    );
    let addrs = addrs(&[("out", 0x20), ("main::1", 0x24), ("main::2", 0x28)]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
fn sub_byte_const_lhs_shift_still_materializes_the_literal() {
    // The const-LHS guard must stay inside the m > 0 arm: a sub-byte
    // const-count shift still routes the operand through
    // `emit_move_val_to_slot`, whose `Val::Const` arm materializes the
    // literal bytes, so `shl i8 7, 2` is 28, not a read of address 0x07.
    let m = parse(
        "global out i8\nfn main(void) ()\n  block entry:\n    %1 = shl i8 7, 2\n    store i8 %1 @out\n    ret void\n",
    );
    let addrs = addrs(&[("out", 0x20), ("main::1", 0x21), ("main::2", 0x22)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.run(200);
    assert_eq!(p.ram()[0x20], 28, "7 << 2 == 28");
    assert!(p.halted());
}

#[test]
fn in_place_i32_shl8_byte_moves_survive_their_own_overwrite() {
    // The `x = x << 8` shape after allocation: the shift's dst slot IS
    // the operand's storage (av == dst, both main::1 and main::2 at
    // 0x24). The copy order is load-bearing: shl must move high-to-low
    // so no source byte is read after the move that overwrites it.
    let m = parse(
        "global a i32\nfn main(void) ()\n  block entry:\n\
         %1 = load i32 @a\n    %2 = shl i32 %1, 8\n    store i32 %2 @a\n    ret void\n",
    );
    let addrs = addrs(&[("a", 0x20), ("main::1", 0x24), ("main::2", 0x24)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let (hi, mid, lo) = (
        asm.find("MOVFF 0x026, 0x027")
            .expect("byte 2 -> byte 3 move"),
        asm.find("MOVFF 0x025, 0x026")
            .expect("byte 1 -> byte 2 move"),
        asm.find("MOVFF 0x024, 0x025")
            .expect("byte 0 -> byte 1 move"),
    );
    assert!(
        hi < mid && mid < lo,
        "in-place shl byte moves must run high-to-low:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    // a = 0x12345678, LE bytes 78 56 34 12.
    p.ram_mut()[0x20] = 0x78;
    p.ram_mut()[0x21] = 0x56;
    p.ram_mut()[0x22] = 0x34;
    p.ram_mut()[0x23] = 0x12;
    p.run(500);
    // a = a << 8 = 0x34567800, LE bytes 00 78 56 34.
    assert_eq!(p.ram()[0x20], 0x00);
    assert_eq!(p.ram()[0x21], 0x78);
    assert_eq!(p.ram()[0x22], 0x56);
    assert_eq!(p.ram()[0x23], 0x34);
    assert!(p.halted());
}

#[test]
fn icmp_eq_materializes_1_when_equal_and_0_when_not() {
    let m = parse("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp eq i8 %1, %2\n    ret void\n");
    let addrs = addrs(&[
        ("a", 0x10),
        ("b", 0x11),
        ("main::1", 0x12),
        ("main::2", 0x13),
        ("main::3", 0x14),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    for (av, bv, expect) in [(5u8, 5u8, 1u8), (5, 6, 0)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = av;
        p.ram_mut()[0x11] = bv;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "eq({av},{bv})");
    }
}

#[test]
fn icmp_ne_distinguishes_equal_from_not_equal() {
    for (a, b, expect) in [(5u8, 5u8, 0u8), (5, 6, 1)] {
        let m = parse("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp ne i8 %1, %2\n    ret void\n");
        let addrs = addrs(&[
            ("a", 0x10),
            ("b", 0x11),
            ("main::1", 0x12),
            ("main::2", 0x13),
            ("main::3", 0x14),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = a;
        p.ram_mut()[0x11] = b;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "ne({a},{b})");
    }
}

#[test]
fn icmp_ult_and_uge_use_the_carry_flag() {
    for (pred, a, b, expect) in [
        ("ult", 3u8, 5u8, 1u8),
        ("ult", 5, 3, 0),
        ("ult", 5, 5, 0),
        ("uge", 5, 3, 1),
        ("uge", 3, 5, 0),
        ("uge", 5, 5, 1),
    ] {
        let m = parse(&format!("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp {pred} i8 %1, %2\n    ret void\n"));
        let addrs = addrs(&[
            ("a", 0x10),
            ("b", 0x11),
            ("main::1", 0x12),
            ("main::2", 0x13),
            ("main::3", 0x14),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = a;
        p.ram_mut()[0x11] = b;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "{pred}({a},{b})");
    }
}

#[test]
fn icmp_ugt_and_ule_combine_c_and_z() {
    for (pred, a, b, expect) in [
        ("ugt", 5u8, 3u8, 1u8),
        ("ugt", 3, 5, 0),
        ("ugt", 5, 5, 0),
        ("ule", 3, 5, 1),
        ("ule", 5, 5, 1),
        ("ule", 5, 3, 0),
    ] {
        let m = parse(&format!("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp {pred} i8 %1, %2\n    ret void\n"));
        let addrs = addrs(&[
            ("a", 0x10),
            ("b", 0x11),
            ("main::1", 0x12),
            ("main::2", 0x13),
            ("main::3", 0x14),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = a;
        p.ram_mut()[0x11] = b;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "{pred}({a},{b})");
    }
}

#[test]
fn icmp_slt_and_sge_use_n_xor_ov() {
    // -1 (0xFF) < 1 is true; 1 < -1 is false. Also cross the signed-
    // overflow boundary: 127 < -128 is false (no overflow in this
    // direction) but tests N!=OV correctly only if the case is chosen so
    // OV actually gets set — include one such case explicitly.
    for (pred, a, b, expect) in [
        ("slt", 0xFFu8, 1u8, 1u8), // -1 < 1
        ("slt", 1, 0xFF, 0),       // 1 < -1 is false
        ("slt", 5, 5, 0),          // equal: strict predicate is false
        ("sge", 1, 0xFF, 1),
        ("sge", 0xFF, 1, 0),
        ("sge", 5, 5, 1),       // equal: non-strict predicate is true
        ("slt", 0x7F, 0x80, 0), // 127 < -128: false, but a-b overflows (OV=1), N must still resolve correctly
    ] {
        let m = parse(&format!("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp {pred} i8 %1, %2\n    ret void\n"));
        let addrs = addrs(&[
            ("a", 0x10),
            ("b", 0x11),
            ("main::1", 0x12),
            ("main::2", 0x13),
            ("main::3", 0x14),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = a;
        p.ram_mut()[0x11] = b;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "{pred}({a:#04x},{b:#04x})");
    }
}

#[test]
fn icmp_sgt_and_sle_combine_z_and_n_xor_ov() {
    for (pred, a, b, expect) in [
        ("sgt", 5u8, 3u8, 1u8),
        ("sgt", 3, 5, 0),
        ("sgt", 5, 5, 0),
        ("sle", 3, 5, 1),
        ("sle", 5, 5, 1),
        ("sle", 5, 3, 0),
    ] {
        let m = parse(&format!("global a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp {pred} i8 %1, %2\n    ret void\n"));
        let addrs = addrs(&[
            ("a", 0x10),
            ("b", 0x11),
            ("main::1", 0x12),
            ("main::2", 0x13),
            ("main::3", 0x14),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = a;
        p.ram_mut()[0x11] = b;
        p.run(200);
        assert_eq!(p.ram()[0x14], expect, "{pred}({a},{b})");
    }
}

#[test]
fn icmp_i16_ties_break_on_the_low_byte() {
    // High bytes equal (0x01), low bytes differ: 0x0105 vs 0x0103.
    for (pred, expect) in [("ult", 0u8), ("ugt", 1), ("eq", 0), ("slt", 0), ("sgt", 1)] {
        let m = parse(&format!("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = icmp {pred} i16 %1, %2\n    ret void\n"));
        let addrs = addrs(&[
            ("a", 0x10),
            ("b", 0x12),
            ("main::1", 0x14),
            ("main::2", 0x16),
            ("main::3", 0x18),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = 0x05; // a lo
        p.ram_mut()[0x11] = 0x01; // a hi
        p.ram_mut()[0x12] = 0x03; // b lo
        p.ram_mut()[0x13] = 0x01; // b hi
        p.run(300);
        assert_eq!(p.ram()[0x18], expect, "{pred}(0x0105, 0x0103)");
    }
}

#[test]
fn icmp_i16_high_byte_alone_decides_when_it_differs() {
    // a=0x00FF, b=0x0100: a < b even though a's low byte is larger.
    let m = parse("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = icmp ult i16 %1, %2\n    ret void\n");
    let addrs = addrs(&[
        ("a", 0x10),
        ("b", 0x12),
        ("main::1", 0x14),
        ("main::2", 0x16),
        ("main::3", 0x18),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x10] = 0xFF;
    p.ram_mut()[0x11] = 0x00;
    p.ram_mut()[0x12] = 0x00;
    p.ram_mut()[0x13] = 0x01;
    p.run(300);
    assert_eq!(p.ram()[0x18], 1);
}

#[test]
fn icmp_i16_full_equality_resolves_correctly_for_every_predicate() {
    // a == b == 0x0142 for all ten predicates: this exercises the "equal
    // at every byte" edge case for both the high-byte compare (where
    // `emit_cmp_branch`'s `l_equal` always defers to the low-byte check)
    // and the low-byte tie-break (where `l_equal` must resolve to this
    // predicate's real answer at full equality — true for the
    // non-strict/eq predicates, false for the strict ones).
    for (pred, expect) in [
        ("eq", 1u8),
        ("ne", 0),
        ("ult", 0),
        ("ule", 1),
        ("ugt", 0),
        ("uge", 1),
        ("slt", 0),
        ("sle", 1),
        ("sgt", 0),
        ("sge", 1),
    ] {
        let m = parse(&format!("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = icmp {pred} i16 %1, %2\n    ret void\n"));
        let addrs = addrs(&[
            ("a", 0x10),
            ("b", 0x12),
            ("main::1", 0x14),
            ("main::2", 0x16),
            ("main::3", 0x18),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        p.ram_mut()[0x10] = 0x42; // a lo
        p.ram_mut()[0x11] = 0x01; // a hi
        p.ram_mut()[0x12] = 0x42; // b lo
        p.ram_mut()[0x13] = 0x01; // b hi
        p.run(300);
        assert_eq!(p.ram()[0x18], expect, "{pred}(0x0142, 0x0142)");
    }
}

#[test]
fn icmp_i16_high_byte_uses_the_predicates_own_signedness() {
    // a=0xFF00, b=0x0100 — the high bytes differ (0xFF vs 0x01) so the
    // high byte alone decides the whole comparison, but the SIGNED and
    // UNSIGNED answers genuinely disagree on this bit pattern: as signed
    // 16-bit values, a = -256 < b = 256, so the signed predicates read
    // a<b; as unsigned 16-bit values, a = 0xFF00 = 65280 > b = 0x0100 =
    // 256, so the unsigned predicates read a>b. If `emit_icmp_i16`'s
    // high-byte compare accidentally used the unsigned tie-break
    // predicate (e.g. `ult` instead of `slt`) instead of `pred` itself,
    // every signed-predicate case below would silently flip. `ult` is
    // included as the control case showing the unsigned reading really
    // is the opposite.
    for (pred, expect) in [("slt", 1u8), ("sle", 1), ("sgt", 0), ("sge", 0), ("ult", 0)] {
        let m = parse(&format!("global a i16\nglobal b i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = load i16 @b\n    %3 = icmp {pred} i16 %1, %2\n    ret void\n"));
        let addrs = addrs(&[
            ("a", 0x10),
            ("b", 0x12),
            ("main::1", 0x14),
            ("main::2", 0x16),
            ("main::3", 0x18),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = 0x00; // a lo
        p.ram_mut()[0x11] = 0xFF; // a hi
        p.ram_mut()[0x12] = 0x00; // b lo
        p.ram_mut()[0x13] = 0x01; // b hi
        p.run(300);
        assert_eq!(p.ram()[0x18], expect, "{pred}(0xFF00, 0x0100)");
    }
}

#[test]
#[should_panic(expected = "const-LHS")]
fn icmp_const_lhs_is_rejected_not_silently_miscompiled() {
    // `val_addr` maps `Val::Const(k)` to `Slot::Direct(k & 0xFF)` — treating
    // a literal as a RAM ADDRESS. Without the guard, `icmp ult i8 5, %x`
    // would silently emit `SUBWF 0x005,W,A`, reading whatever byte lives at
    // address 0x05 instead of using the literal 5. This must fail loudly
    // instead, matching the `Inst::Bin` const-LHS guard.
    let m = parse(
        "global x i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @x\n    %2 = icmp ult i8 5, %1\n    ret void\n",
    );
    let addrs = addrs(&[("x", 0x10), ("main::1", 0x12), ("main::2", 0x13)]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
#[should_panic(expected = "const-LHS")]
fn icmp_i16_const_lhs_is_rejected_not_silently_miscompiled() {
    // Same hazard as `icmp_const_lhs_is_rejected_not_silently_miscompiled`,
    // but for the new i16 path (`emit_icmp_i16`) — the guard must not be
    // bypassed just because the width changed.
    let m = parse(
        "global x i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @x\n    %2 = icmp ult i16 5, %1\n    ret void\n",
    );
    let addrs = addrs(&[("x", 0x10), ("main::1", 0x12), ("main::2", 0x14)]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
fn zext_i8_to_i16_zero_fills_the_high_byte() {
    let m = parse("global a i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = zext i8 %1 to i16\n    ret void\n");
    let addrs = addrs(&[("a", 0x10), ("main::1", 0x11), ("main::2", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x10] = 0xFF;
    p.run(50);
    assert_eq!(p.ram()[0x12], 0xFF);
    assert_eq!(p.ram()[0x13], 0x00);
}

#[test]
fn zext_i1_to_i8_same_width_widen_compiles_and_runs() {
    // Regression test: `zext i1 to i8` (e.g. `u8 b = (a < b);`) is legal
    // and common — i1 and i8 both report `.bytes() == 1` in the byte
    // model, so this is a same-width "widen" that's really a 1-byte copy.
    // A prior version of this guard required `to.bytes() > from.bytes()`
    // (strictly wider), which panicked on this case even though it's not
    // a narrowing bug. The guard must accept `to.bytes() >= from.bytes()`.
    let m = parse(
        "global a i8\nglobal b i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp eq i8 %1, %2\n    %4 = zext i1 %3 to i8\n    store i8 %4 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x10),
        ("b", 0x11),
        ("out", 0x12),
        ("main::1", 0x13),
        ("main::2", 0x14),
        ("main::3", 0x15),
        ("main::4", 0x16),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    for (av, bv, expect) in [(5u8, 5u8, 1u8), (5, 6, 0)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = av;
        p.ram_mut()[0x11] = bv;
        p.run(200);
        assert_eq!(p.ram()[0x12], expect, "zext(icmp eq({av},{bv}))");
    }
}

#[test]
fn sext_i8_to_i16_sign_fills_the_high_byte() {
    let m = parse("global a i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = sext i8 %1 to i16\n    ret void\n");
    let addrs = addrs(&[("a", 0x10), ("main::1", 0x11), ("main::2", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x10] = 0xFF; // -1
    p.run(50);
    assert_eq!(p.ram()[0x12], 0xFF);
    assert_eq!(p.ram()[0x13], 0xFF, "sign-filled");
}

#[test]
fn trunc_i16_to_i8_keeps_the_low_byte() {
    let m = parse("global a i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = trunc i16 %1 to i8\n    ret void\n");
    let addrs = addrs(&[("a", 0x10), ("main::1", 0x12), ("main::2", 0x14)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x10] = 0x34;
    p.ram_mut()[0x11] = 0x12;
    p.run(50);
    assert_eq!(p.ram()[0x14], 0x34);
}

#[test]
#[should_panic(expected = "const source Zext")]
fn zext_const_source_is_rejected_not_silently_miscompiled() {
    // `val_addr` maps `Val::Const(k)` to `Slot::Direct(k & 0xFF)` — treating
    // a literal as a RAM ADDRESS. Without the guard, `zext i8 5 to i16`
    // would silently `MOVFF` from whatever byte lives at address 0x05
    // instead of using the literal 5. This must fail loudly instead,
    // matching the `Inst::Bin`/`Inst::Icmp` const-LHS guards.
    let m = parse("fn main(void) ()\n  block entry:\n    %1 = zext i8 5 to i16\n    ret void\n");
    let addrs = addrs(&[("main::1", 0x12)]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
#[should_panic(expected = "const source Sext")]
fn sext_const_source_is_rejected_not_silently_miscompiled() {
    // Same hazard as `zext_const_source_is_rejected_not_silently_miscompiled`.
    let m = parse("fn main(void) ()\n  block entry:\n    %1 = sext i8 5 to i16\n    ret void\n");
    let addrs = addrs(&[("main::1", 0x12)]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
#[should_panic(expected = "const source Trunc")]
fn trunc_const_source_is_rejected_not_silently_miscompiled() {
    // Same hazard as `zext_const_source_is_rejected_not_silently_miscompiled`.
    let m = parse("fn main(void) ()\n  block entry:\n    %1 = trunc i16 5 to i8\n    ret void\n");
    let addrs = addrs(&[("main::1", 0x11)]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
#[should_panic(expected = "const cond Select")]
fn select_const_cond_is_rejected_not_silently_miscompiled() {
    // `emit_load_w`'s `Val::Const` arm emits only `MOVLW`, which (per this
    // project's simulator, `crates/sim/src/lib.rs:903`) never touches the
    // Z flag — unlike `MOVF` (the `Val::Reg`/`Val::Global` arm), which
    // does via `set_zn` (`crates/sim/src/lib.rs:779-783`). Without the
    // guard, `select i1 1 ...`'s `BZ` right after the `MOVLW` would test
    // whatever Z flag the PREVIOUS instruction happened to leave, silently
    // picking the wrong side of the `Select` instead of using the literal
    // cond. This must fail loudly instead, matching the `Inst::Bin`/
    // `Inst::Icmp` const-LHS guards and `Zext`/`Sext`/`Trunc`'s
    // const-source guards.
    let m =
        parse("fn main(void) ()\n  block entry:\n    %1 = select i1 1 i8 5 i8 6\n    ret void\n");
    let addrs = addrs(&[("main::1", 0x12)]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
fn select_picks_a_when_cond_is_true_and_b_otherwise() {
    let m = parse("global c i8\nglobal a i8\nglobal b i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @c\n    %2 = load i8 @a\n    %3 = load i8 @b\n    %4 = icmp ne i8 %1, 0\n    %5 = select i1 %4 i8 %2 i8 %3\n    ret void\n");
    let addrs = addrs(&[
        ("c", 0x10),
        ("a", 0x11),
        ("b", 0x12),
        ("main::1", 0x13),
        ("main::2", 0x14),
        ("main::3", 0x15),
        ("main::4", 0x16),
        ("main::5", 0x17),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    for (c, expect) in [(1u8, 0x11u8), (0, 0x22)] {
        // reuse fresh ram each run
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = c;
        p.ram_mut()[0x11] = 0x11;
        p.ram_mut()[0x12] = 0x22;
        p.run(200);
        assert_eq!(p.ram()[0x17], expect, "select(cond={c})");
    }
}

#[test]
fn br_unconditionally_jumps_to_the_target_block() {
    let m = parse("fn main(void) ()\n  block entry:\n    br skip\n  block skip:\n    ret void\n");
    let asm = select(&PIC18F4550, &m, &addrs(&[]), None);
    // Index-based label scheme (matches isel::select exactly): the first
    // block ("entry", here) is the bare function name; every other block
    // is `{func}_L{label}` — "skip" is the second block, so `main_Lskip`.
    assert!(asm.contains("BRA main_Lskip"), "asm:\n{asm}");
    assert!(
        asm.contains("main_Lskip:"),
        "target block must be labeled:\n{asm}"
    );
}

#[test]
fn brcond_branches_on_the_condition_byte() {
    // Each target block stores a DISTINGUISHABLE value to @out (matching
    // the pattern `select_picks_a_when_cond_is_true_and_b_otherwise` uses
    // for `Select`) so this test actually proves which way `BZ`/`BRA`
    // branch, not just that some path halts without panicking — a
    // polarity inversion (`BZ` going to the wrong target) would still
    // pass a "both paths halt" check but fails this one.
    let m = parse("global c i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @c\n    br i1 %1 t f\n  block t:\n    store i8 1 @out\n    ret void\n  block f:\n    store i8 2 @out\n    ret void\n");
    let addrs = addrs(&[("c", 0x10), ("out", 0x11), ("main::1", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    for (c, expect) in [(1u8, 1u8), (0, 2)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = c;
        p.run(200);
        assert!(p.halted());
        assert_eq!(
            p.ram()[0x11],
            expect,
            "brcond(cond={c}) took the wrong target"
        );
    }
}

#[test]
fn phi_copies_the_incoming_value_before_the_predecessor_blocks_terminator() {
    // The brief's Step-1 code for this test used a literal `br i1 1 a b`
    // cond, but that trips the const-cond guard added below (Concern #3 of
    // this task: BrCond with a `Val::Const` cond would silently branch on a
    // stale Z flag, exactly the `Select`-cond hazard already fixed in Task
    // 11). Routed through a loaded register instead so this test exercises
    // only what it's meant to (Phi copies landing in both predecessor
    // blocks), not the guard.
    let m = parse(
        "global c i8\nfn main(void) ()\n\
         block entry:\n\
           %1 = load i8 @c\n\
           br i1 %1 a b\n\
         block a:\n\
           br j\n\
         block b:\n\
           br j\n\
         block j:\n\
           %2 = phi i8 5 a 7 b\n\
           ret void\n",
    );
    let addrs = addrs(&[("c", 0x10), ("main::1", 0x11), ("main::2", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    // The copy into %1's slot must appear in BOTH predecessor blocks
    // (block a gets MOVLW 5, block b gets MOVLW 7), before each one's own
    // `br j`. Blocks "a"/"b"/"j" are all non-first blocks here (block
    // "entry" is first), so each gets a `main_L{label}:` symbol.
    assert!(asm.contains("main_La:"), "asm:\n{asm}");
    assert!(asm.contains("main_Lb:"), "asm:\n{asm}");
    assert!(asm.contains("main_Lj:"), "asm:\n{asm}");
    assert!(asm.contains("MOVLW 0x05"), "asm:\n{asm}");
    assert!(asm.contains("MOVLW 0x07"), "asm:\n{asm}");
    // Bare `asm.contains(...)` above doesn't confirm WHICH block each copy
    // landed in — split the asm on block labels and check per-section, so
    // a copy landing in the wrong predecessor's section (or in neither)
    // would fail this test even though the whole-file `contains` checks
    // above would still pass.
    let a_section = block_section(&asm, "main_La");
    let b_section = block_section(&asm, "main_Lb");
    assert!(
        a_section.contains("MOVLW 0x05"),
        "block a's section:\n{a_section}"
    );
    assert!(
        !a_section.contains("MOVLW 0x07"),
        "block a's section must not contain b's phi copy:\n{a_section}"
    );
    assert!(
        b_section.contains("MOVLW 0x07"),
        "block b's section:\n{b_section}"
    );
    assert!(
        !b_section.contains("MOVLW 0x05"),
        "block b's section must not contain a's phi copy:\n{b_section}"
    );
}

/// The lines belonging to one block's label, up to (but excluding) the
/// next line that looks like a label (ends with `:`). Used to check that
/// per-predecessor phi copies land in the RIGHT block's section, not just
/// somewhere in the whole function's asm.
fn block_section(asm: &str, label: &str) -> String {
    let marker = format!("{label}:");
    let lines: Vec<&str> = asm.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim() == marker)
        .unwrap_or_else(|| panic!("label {label} not found in:\n{asm}"));
    lines[start + 1..]
        .iter()
        .take_while(|l| !l.trim_end().ends_with(':'))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ret_with_a_value_writes_it_into_the_fixed_retval_region() {
    // `Ret(Some((ty, v)))` is new in this task — before this task, only
    // `Ret(None)` was handled at all (inside `emit_inst`, since it hadn't
    // moved into the terminator pass yet). It writes each byte of the
    // returned value into the fixed retval region (`device.fixed_retval`,
    // which is 0x0000 for `PIC18F4550`) before `RETURN`. Check both an i8
    // and an i16 return so both the single-byte and multi-byte loop paths
    // are exercised.
    let m = parse("fn main(void) ()\n  block entry:\n    ret i8 42\n");
    let asm = select(&PIC18F4550, &m, &addrs(&[]), None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.run(200);
    assert!(p.halted());
    assert_eq!(
        p.ram()[0x00],
        42,
        "i8 retval should land at the fixed retval region (0x0000)"
    );

    let m16 = parse("fn main(void) ()\n  block entry:\n    ret i16 4660\n"); // 4660 == 0x1234
    let asm16 = select(&PIC18F4550, &m16, &addrs(&[]), None);
    let words16 = asm::assemble_pic18(&asm16);
    let mut p16 = pic14_sim::Pic18::new(words16);
    p16.run(200);
    assert!(p16.halted());
    assert_eq!(p16.ram()[0x00], 0x34, "i16 retval low byte");
    assert_eq!(p16.ram()[0x01], 0x12, "i16 retval high byte");
}

#[test]
fn rotated_loop_exit_phi_reads_the_pre_increment_value_not_the_clobbered_one() {
    // Regression for the Critical bug found in task review of the first
    // Task 12 implementation: phi copies were keyed by PREDECESSOR alone,
    // so a `BrCond` whose two successors both consume phis ran BOTH
    // successors' copies unconditionally before the branch. On this
    // rotated-loop shape (the standard clang -O1 loop shape, and the one
    // `scalar.c` — a Task 15 e2e fixture — actually produces), that
    // clobbers the loop header's phi slot (`%2`) with the next-iteration
    // value BEFORE the exit block's phi (`%5`) gets a chance to read the
    // CURRENT one, so the exit block would read a wrong, clobbered value.
    //
    // Loop shape: `%2` (the header phi) starts at 0 (from `entry`), and
    // each iteration through `body` computes `%3 = %2 + 1`, loops back
    // while `%3 < 3` (feeding `%2 <- %3` on that back edge), and exits
    // once `%3 == 3` (feeding `%5 <- %2`, the CURRENT value BEFORE this
    // iteration's increment, into the exit block).
    //
    // Trace: iter1 %2=0,%3=1,cont(1<3); iter2 %2=1,%3=2,cont(2<3);
    // iter3 %2=2,%3=3,exit(3<3 false) -> %5 must be 2 (the value %2 held
    // going into the FINAL iteration), not 3 (the clobbered post-increment
    // value the original bug would have produced).
    let m = parse(
        "global out i8\nfn main(void) ()\n\
         block entry:\n\
           br body\n\
         block body:\n\
           %2 = phi i8 0 entry %3 body\n\
           %3 = add i8 %2, 1\n\
           %4 = icmp ult i8 %3, 3\n\
           br i1 %4 body exit\n\
         block exit:\n\
           %5 = phi i8 %2 body\n\
           store i8 %5 @out\n\
           ret void\n",
    );
    let addrs = addrs(&[
        ("out", 0x10),
        ("main::2", 0x11),
        ("main::3", 0x12),
        ("main::4", 0x13),
        ("main::5", 0x14),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.run(500);
    assert!(p.halted());
    assert_eq!(p.ram()[0x10], 2, "exit's phi must read %2's pre-increment value (2), not the clobbered next-iteration value (3)");
}

#[test]
fn brcond_both_edges_phi_copies_get_correct_bsr_after_the_synthesized_fcopies_label() {
    // Regression for a bug this fix round's own `(Some(ct), Some(cf))`
    // BrCond phi-copy handling introduced: the synthesized `l_fcopies`
    // label (reached via `BZ`, not a real block label) didn't reset
    // `Gen::bsr`, so if the t-edge's copy left the TRACKED `bsr` pointing
    // at bank 2, the f-edge's copy — which never actually runs the
    // t-edge's `MOVLB` at runtime when `BZ` is taken — would wrongly
    // believe BSR was already 2 and skip emitting its own `MOVLB`,
    // writing to the wrong physical bank.
    //
    // `main::1` (the cond, read from `%1`'s slot) sits in bank 1; both
    // phi destinations (`main::2` for the t edge, `main::3` for the f
    // edge) sit in bank 2 — the shape needed to trigger the hazard: the
    // cond load leaves REAL BSR=1, the t-edge's copy sets REAL BSR=2 only
    // when the t-edge actually executes, and the f-edge's copy must
    // independently re-establish BSR=2 rather than trusting a stale
    // tracked value left over from code-generation order.
    let m = parse(
        "global cond i8\nglobal outT i8\nglobal outF i8\nfn main(void) ()\n\
         block entry:\n\
           %1 = load i8 @cond\n\
           br i1 %1 t f\n\
         block t:\n\
           %2 = phi i8 7 entry\n\
           store i8 %2 @outT\n\
           ret void\n\
         block f:\n\
           %3 = phi i8 9 entry\n\
           store i8 %3 @outF\n\
           ret void\n",
    );
    let addrs = addrs(&[
        ("cond", 0x10),
        ("outT", 0x11),
        ("outF", 0x12),
        ("main::1", 0x101),
        ("main::2", 0x210),
        ("main::3", 0x211),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    for (c, expect_t, expect_f) in [(1u8, 7u8, 0u8), (0, 0, 9)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = c;
        p.run(200);
        assert!(p.halted());
        assert_eq!(
            p.ram()[0x11],
            expect_t,
            "outT (bank-2 phi dest) after cond={c}"
        );
        assert_eq!(p.ram()[0x12], expect_f, "outF (bank-2 phi dest) after cond={c} — without the BSR reset at l_fcopies, this lands in the wrong bank on the cond=0 path");
    }
}

#[test]
fn runtime_inttoptr_derefs_through_fsr0_indf0() {
    // epic-cc#117 shape 1 on PIC18: the address bytes land in the dst slot
    // and the volatile load goes through FSR0/INDF0 (ADR-009's pointer
    // model), no BSR/MOVLB involvement for the indirect access.
    let m = parse(
        "global off i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %o = load i8 @off\n    %a = inttoptr i16 %o to i16\n    %v = load i8 %a\n    store i8 %v @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("off", 0x20),
        ("out", 0x21),
        ("main::o", 0x25),
        ("main::a", 0x26),
        ("main::v", 0x27),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVFF 0x026, 0xFE9") && asm.contains("MOVFF 0x027, 0xFEA"),
        "FSR0 loaded from the address slot:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFEF, 0x027"),
        "access through INDF0 (0xFEF):\n{asm}"
    );
}

#[test]
fn runtime_ptr_select_derefs_through_fsr0() {
    // The HAL's const-arm pointer select on PIC18: the chosen address's two
    // bytes land in the dst slot and the deref goes through FSR0/INDF0.
    let m = parse("global c i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %c = load i8 @c\n    %p = select i1 %c ptr 12 ptr 13\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n");
    let addrs = addrs(&[
        ("c", 0x20),
        ("out", 0x21),
        ("main::c", 0x25),
        ("main::p", 0x26),
        ("main::v", 0x27),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVFF 0x026, 0xFE9"),
        "FSR0 loaded from the address slot:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFEF, 0x027"),
        "access through INDF0 (0xFEF):\n{asm}"
    );
}

#[test]
fn runtime_ptr_phi_derefs_through_slot_after_phi_copies() {
    // The GetFlag -O1 shape on PIC18: a pointer phi joining a literal-arm
    // select result and a literal. Phi copies move the incoming's address
    // bytes into the dst slot per edge, then the deref goes indirect
    // through FSR0/INDF0 from the slot.
    let m = parse(
        "global c i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    %c = load i8 @c\n    br i1 %c t f\n  block t:\n    %pt = select i1 %c ptr 12 ptr 13\n    br merge\n  block f:\n    br merge\n  block merge:\n    %p = phi ptr %pt t 11 f\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("c", 0x20),
        ("out", 0x21),
        ("main::c", 0x25),
        ("main::pt", 0x26),
        ("main::p", 0x28),
        ("main::v", 0x29),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVFF 0x028, 0xFE9"),
        "FSR0 loaded from the phi dst slot:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFEF, 0x029"),
        "access through INDF0 (0xFEF):\n{asm}"
    );
}

#[test]
fn select_l_end_resets_bsr_so_a_later_same_block_instruction_is_not_misbanked() {
    // Regression for a third instance of the same BSR-tracking hazard,
    // this time at `Select`'s MERGE label (`l_end`), found during the
    // systemic audit that added `Gen::emit_label`. `l_end` is reached two
    // ways: `BRA l_end` from the `a`-arm, and plain fallthrough from the
    // `b`-arm — and the two arms don't necessarily touch `bsr` the same
    // way. Here `s.a` is a REGISTER value (copied via `MOVFF`, which never
    // calls `operand()`/touches `bsr` at all), while `s.b` is a CONSTANT
    // (copied via `MOVLW`+`MOVWF`, which does call `operand()` for the
    // (shared) destination's bank). So after generating the whole
    // `Select`, the tracked `bsr` reflects the `b`-arm's bank
    // unconditionally — correct if the `b`-arm is the one that actually
    // ran, wrong if the `a`-arm ran instead (real BSR there is whatever
    // it was before the `Select`, since the `a`-arm's `MOVFF` never
    // touched it).
    //
    // `@out` is placed in the SAME bank as the `Select`'s destination
    // (bank 2) so the stale tracked value from the `b`-arm coincidentally
    // "matches" what `@out`'s store needs — exactly the condition needed
    // to make `operand()` wrongly skip the `MOVLB` on the `a`-path.
    let m = parse(
        "global cond i8\nglobal r i8\nglobal out i8\nfn main(void) ()\n\
         block entry:\n\
           %1 = load i8 @cond\n\
           %2 = load i8 @r\n\
           %3 = select i1 %1 i8 %2 i8 9\n\
           store i8 5 @out\n\
           ret void\n",
    );
    let addrs = addrs(&[
        ("cond", 0x10),
        ("r", 0x11),
        ("out", 0x211),     // bank 2, same bank as main::3 below
        ("main::1", 0x101), // bank 1: the cond load's own MOVLB
        ("main::2", 0x12),
        ("main::3", 0x210), // bank 2: the Select's dst (b-arm's MOVLB target)
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    for c in [1u8, 0] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        p.ram_mut()[0x10] = c;
        p.ram_mut()[0x11] = 0x55;
        p.run(200);
        assert!(p.halted());
        assert_eq!(
            p.ram()[0x211], 5,
            "store after the Select (cond={c}) must land in @out's real bank-2 address, not get silently misbanked by a stale tracked BSR left over from Select's b-arm"
        );
    }
}

#[test]
#[should_panic(expected = "const cond BrCond")]
fn brcond_const_cond_is_rejected_not_silently_miscompiled() {
    // Same hazard as `select_const_cond_is_rejected_not_silently_miscompiled`:
    // `emit_load_w`'s `Val::Const` arm emits only `MOVLW`, which (per this
    // project's simulator, `crates/sim/src/lib.rs:903`) never touches the
    // Z flag. Without the guard, `br i1 1 ...`'s `BZ` right after the
    // `MOVLW` would test whatever Z flag the PREVIOUS instruction happened
    // to leave, silently branching to the wrong target instead of using
    // the literal cond. This must fail loudly instead.
    let m = parse("fn main(void) ()\n  block entry:\n    br i1 1 t f\n  block t:\n    ret void\n  block f:\n    ret void\n");
    let addrs = addrs(&[]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
fn call_copies_scalar_args_and_reads_the_retval_back() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             %1 = call i8 @add1(i8 5)\n\
             ret void\n\
         fn add1(i8) (x=i8)\n\
           block entry:\n\
             %2 = add i8 %x, 1\n\
             ret i8 %2\n",
    );
    let addrs = addrs(&[("main::1", 0x10), ("add1::x", 0x11), ("add1::2", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.run(300);
    assert_eq!(p.ram()[0x10], 6, "main::1 gets add1(5)'s retval");
}

#[test]
fn call_return_invalidates_tracked_bsr_so_a_later_banked_access_is_not_misbanked() {
    // Regression for the final-review finding: `Gen.bsr` tracks the bank
    // the MOST RECENT *emitted* `MOVLB` set, but a `CALL` transfers control
    // to a callee that runs its own arbitrary `MOVLB`s and never restores
    // the caller's bank on `RETURN` — so the tracked value is stale the
    // instant control returns to the caller, and (unless invalidated)
    // `operand()` can wrongly elide a `MOVLB` the next banked access
    // actually needs.
    //
    // Shape, closely modeled on the reviewer's repro:
    //   main: MOVLB 0x1 (tracked bsr = Some(1)), ADDWF/MOVWF against bank-1
    //         locals (`main::1`/`main::2`/`main::3`, all >= 0x100) ->
    //         CALL f
    //   f:    its own ADDWF/MOVWF against BANK-2 locals (`f::1`/`f::2`/
    //         `f::3`, all >= 0x200) -> emits its own MOVLB 0x2, and never
    //         restores bank 1 before RETURN, so the REAL hardware BSR is 2
    //         when control returns to main.
    //   main: a second `add` reusing %1/%2 (bank-1 addresses again),
    //         storing into `main::4` (also bank 1).
    //
    // Without the fix, main's tracked `bsr` is still `Some(1)` after the
    // `CALL` (never invalidated) — and the post-call add's operands are
    // ALSO bank 1, so `operand()` sees tracked==target and wrongly skips
    // the `MOVLB`. But the REAL BSR at that point is 2 (left over from
    // `f`), so every banked access in the post-call add actually lands in
    // bank 2 (`f`'s slots) instead of bank 1 (main's own `%1`/`%2`/
    // `main::4`) — silently reading `f`'s operands and writing `main::4`'s
    // result into `f::3`'s address (0x213) instead of `main::4`'s real
    // address (0x113), leaving `main::4` untouched (0x00).
    //
    // With the fix, `self.bsr = None` after `CALL` forces a fresh `MOVLB
    // 0x1` before the post-call add touches anything, so it correctly
    // reads main's own bank-1 `%1`/`%2` and writes bank-1 `main::4`.
    let m = parse(
        "global a i8\nglobal b i8\nglobal c i8\nglobal d i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @a\n\
             %2 = load i8 @b\n\
             %3 = add i8 %1, %2\n\
             call void @f()\n\
             %4 = add i8 %1, %2\n\
             ret void\n\
         fn f(void) ()\n\
           block entry:\n\
             %1 = load i8 @c\n\
             %2 = load i8 @d\n\
             %3 = add i8 %1, %2\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x21),
        ("c", 0x22),
        ("d", 0x23),
        ("main::1", 0x110), // bank 1
        ("main::2", 0x111), // bank 1
        ("main::3", 0x112), // bank 1: pre-call add's dst, forces MOVLB 0x1
        ("main::4", 0x113), // bank 1: post-call add's dst — the one under test
        ("f::1", 0x210),    // bank 2
        ("f::2", 0x211),    // bank 2
        ("f::3", 0x212),    // bank 2: f's own add dst, forces MOVLB 0x2 for real
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x20] = 3; // a
    p.ram_mut()[0x21] = 4; // b
    p.ram_mut()[0x22] = 0x11; // c (f's own operands; must not leak into main::4)
    p.ram_mut()[0x23] = 0x22; // d
    p.run(500);
    assert!(p.halted(), "asm:\n{asm}");
    assert_eq!(
        p.ram()[0x112],
        7,
        "sanity: the pre-call add (main::3 = a+b) must still be correct:\nasm:\n{asm}"
    );
    assert_eq!(
        p.ram()[0x113],
        7,
        "post-call add (main::4 = a+b, bank 1) must land at its real bank-1 \
         address using main's OWN operands, not get silently misbanked into \
         bank 2 (f's slots) by a stale tracked BSR left over from the CALL:\nasm:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "const byval call arg")]
fn call_const_byval_arg_is_rejected_not_silently_miscompiled() {
    // A `byval` arg means "copy N bytes from the address `arg.val` points
    // to" — the IR's text parser has no type-level restriction preventing
    // a bare literal there (`parse_call_arg` accepts any `parse_val`
    // result after the `byvalN` keyword), and `val_addr`'s `Val::Const`
    // arm would silently reinterpret the literal as a RAM address
    // (`k & 0xFF`) rather than reject it. Same hazard class as `Bin`'s
    // const-LHS guard; must fail loudly instead.
    let m = parse(
        "fn main(void) ()\n  block entry:\n    call void @f(byval2 5)\n    ret void\n\
         fn f(void) (p=byval2)\n  block entry:\n    ret void\n",
    );
    let addrs = addrs(&[("f::p", 0x10)]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
fn a_byval_arg_through_a_geped_pointer_copies_from_the_right_offset() {
    // Passing &g.field (a struct-field GEP) as a byval arg must copy from
    // g's base + the field's offset, not from g's base.
    let m = parse(
        "global g i8\n\
         fn f(void) (p=byval2)\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %fp = gep @g +2\n\
             call void @f(byval2 %fp)\n\
             ret void\n",
    );
    let addrs = addrs(&[("g", 0x100), ("f::p", 0x120)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVFF 0x102, 0x120") || asm.contains("MOVFF 0x102,0x120"),
        "the byval copy must start at g+2 (0x102), not g's base (0x100):\n{asm}"
    );
}

#[test]
#[should_panic(expected = "const sret call arg")]
fn call_const_sret_arg_is_rejected_not_silently_miscompiled() {
    // Same hazard as the byval case above: an `sret` arg is always meant
    // to be a 2-byte pointer, but the IR's text parser doesn't prevent a
    // bare literal after the `sret` keyword, and `val_addr`'s
    // `Val::Const` arm would silently treat the literal as an address
    // instead of rejecting it.
    let m = parse(
        "fn main(void) ()\n  block entry:\n    call void @f(sret 5)\n    ret void\n\
         fn f(void) (p=sret)\n  block entry:\n    ret void\n",
    );
    let addrs = addrs(&[("f::p", 0x10)]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
fn an_sret_arg_through_a_geped_pointer_writes_the_target_address_into_the_callees_slot() {
    // `call void @f(sret %p)` with `%p = gep @g +0`: the caller-side sret
    // arm must write the 2-byte ADDRESS of g (not g's contents) into the
    // callee's sret slot, low byte first, via MOVLW/MOVWF. g at 0x100 ->
    // MOVLW 0x00 / MOVWF slot, then MOVLW 0x01 / MOVWF slot+1.
    let m = parse(
        "global g i8\n\
         fn f(void) (r=sret)\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %p = gep @g +0\n\
             call void @f(sret %p)\n\
             ret void\n",
    );
    let addrs = addrs(&[("g", 0x100), ("f::r", 0x120)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVLW 0x00") && asm.contains("MOVWF 0x020,B"),
        "the sret slot's low byte must receive g's low address byte (0x00):\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0x01") && asm.contains("MOVWF 0x021,B"),
        "the sret slot's high byte must receive g's high address byte (0x01):\n{asm}"
    );
}

#[test]
fn a_gep_with_a_constant_offset_and_no_dynamic_term_loads_directly() {
    // arr[2] with a CONST index folds to a plain direct address, no FSR,
    // no LFSR, just a MOVFF at the folded address (base + 2).
    let m = parse(
        "global arr i8\nglobal out i8\nfn main(void) ()\n  block entry:\n\
           %p = gep @arr +2\n    %v = load i8 %p\n    store i8 %v @out\n    ret void\n",
    );
    let addrs = addrs(&[("arr", 0x100), ("out", 0x110), ("main::v", 0x111)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVFF 0x102, 0x111") || asm.contains("MOVFF 0x102,0x111"),
        "arr[2] must read directly from base+2 (0x102), no FSR machinery:\n{asm}"
    );
    assert!(
        // Scoped to main's body: `__start`'s zero-clear (epic-cc#561)
        // contributes its own LFSR-seeded loop.
        !asm.split("__start:").next().unwrap().contains("LFSR"),
        "a fully-constant GEP needs no FSR setup in main's body:\n{asm}"
    );
}

#[test]
fn a_dynamic_index_sets_fsr0_and_reads_through_indf0() {
    // ram[i]: base = @ram (0x120), k = 0, terms = [(1, "i")] (scale 1,
    // a byte array). Must LFSR the base then read through INDF0, no
    // constant-offset direct MOVFF this time.
    let m = parse(
        "global ram i8\n\
         global out i8\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %p = gep @ram +0 +1*%i\n\
             %v = load i8 %p\n\
             store i8 %v @out\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("ram", 0x120),
        ("out", 0x130),
        ("idx", 0x131),
        ("main::i", 0x132),
        ("main::v", 0x133),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("LFSR 0, 0x120") || asm.contains("LFSR 0,0x120"),
        "must seed FSR0 with the array base:\n{asm}"
    );
    assert!(
        asm.contains("0xFEF") || asm.contains("INDF0"),
        "must read through INDF0:\n{asm}"
    );
}

#[test]
fn a_scale_2_dynamic_index_scales_through_mulwf() {
    // `ram16[i]` with element width 2: the offset is 2*i. A scale-2 term
    // takes the MULWF form (6 words) over the two unrolled
    // ADDWF-onto-FSR0L adds (8 words) on any PIC18 with the multiplier.
    // (epic-cc#477)
    let m = parse(
        "global ram16 i16\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %p = gep @ram16 +0 +2*%i\n\
             %v = load i16 %p\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("ram16", 0x140),
        ("idx", 0x150),
        ("main::i", 0x151),
        ("main::v", 0x152),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVLW 0x02") && asm.contains("MULWF"),
        "a scale-2 term must scale through MULWF:\n{asm}"
    );
    assert!(
        asm.contains("MOVF 0xFF3,W,A") && asm.contains("MOVF 0xFF4,W,A"),
        "the product must fold onto the FSR0 pair:\n{asm}"
    );
}

#[test]
fn a_two_term_dynamic_gep_accumulates_both_terms_onto_fsr0() {
    // epic-cc#442: a GEP with two register-indexed dynamic terms (the
    // doubly-indexed shape, e.g. `&arr[i][j]` or `&arr[i].field[j]` after
    // GEP-chain folding) must add BOTH terms onto FSR0L/FSR0H, not just
    // the first (dropping a term silently mis-addresses, ADR-009's
    // concern). Formerly a documented P3 scope boundary that panicked
    // (`a_two_term_dynamic_gep_panics_loudly`); now supported by looping
    // `add_term_to_fsr0` over every term, mirroring PIC14 isel's
    // `emit_accum_terms`.
    let m = parse(
        "global arr i8\n\
         global idx1 i8\n\
         global idx2 i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx1\n\
             %j = load i8 @idx2\n\
             %p = gep @arr +0 +1*%i +1*%j\n\
             %v = load i8 %p\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("arr", 0x120),
        ("idx1", 0x130),
        ("idx2", 0x131),
        ("main::i", 0x132),
        ("main::j", 0x133),
        ("main::v", 0x134),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("LFSR 0, 0x120") || asm.contains("LFSR 0,0x120"),
        "must seed FSR0 with the array base before accumulating terms:\n{asm}"
    );
    assert!(
        asm.contains("0x132") && asm.contains("0x133"),
        "both dynamic terms (%i at 0x132, %j at 0x133) must be read and added, not just the first:\n{asm}"
    );
    let addwf_to_fsr0l = asm.matches("ADDWF 0x0E9").count() + asm.matches("ADDWF 0x0e9").count();
    assert!(
        addwf_to_fsr0l >= 2,
        "two scale-1 terms must unroll two adds onto FSR0L:\n{asm}"
    );
}

#[test]
fn a_three_term_dynamic_gep_is_not_hardcoded_to_two() {
    // Guard against a fix that special-cases exactly two terms (the
    // minimum repro epic-cc#442 needed): a third dynamic term must also
    // compile and accumulate onto FSR0.
    let m = parse(
        "global arr i8\n\
         global idx1 i8\n\
         global idx2 i8\n\
         global idx3 i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx1\n\
             %j = load i8 @idx2\n\
             %k = load i8 @idx3\n\
             %p = gep @arr +0 +1*%i +1*%j +1*%k\n\
             %v = load i8 %p\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("arr", 0x120),
        ("idx1", 0x130),
        ("idx2", 0x131),
        ("idx3", 0x135),
        ("main::i", 0x132),
        ("main::j", 0x133),
        ("main::k", 0x136),
        ("main::v", 0x134),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let addwf_to_fsr0l = asm.matches("ADDWF 0x0E9").count() + asm.matches("ADDWF 0x0e9").count();
    assert!(
        addwf_to_fsr0l >= 3,
        "three scale-1 terms must unroll three adds onto FSR0L:\n{asm}"
    );
}

#[test]
fn an_sret_return_writes_through_the_callers_address() {
    // struct Pair mk(...) { r.a = a; return r; }: inside mk, `r` is an
    // sret param: its SLOT holds the caller's target address, and every
    // field store through it must go via FSR0/INDF0, never a direct
    // write to the slot's own address (that would corrupt the pointer).
    let m = parse(
        "fn mk(void) (r=sret)\n\
           block entry:\n\
             store i8 5 %r\n\
             ret void\n",
    );
    let addrs = addrs(&[("mk::r", 0x160)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("LFSR 0, 0x160") && !asm.contains("LFSR 0,0x160"),
        "the store target is INSIDE the pointer at 0x160, not the literal address 0x160:\n{asm}"
    );
    assert!(
        asm.contains("0xFEF") || asm.contains("INDF0"),
        "an sret store must go through INDF0:\n{asm}"
    );
}

#[test]
fn a_const_length_memcpy_copies_byte_by_byte() {
    // Whole-struct assignment (`g = mk(...)` in structs.c) lowers to a
    // constant-length memcpy: three MOVFF src+i -> dst+i byte copies, no
    // loop and no FSR for direct global addresses (mirrors PIC14's
    // `memcpy_emits_byte_pairs`, crates/isel/tests/isel.rs).
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 3\n\
             ret void\n",
    );
    let addrs = addrs(&[("src", 0x100), ("dst", 0x110)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    for i in 0..3u16 {
        let expect = format!("MOVFF 0x{:03X}, 0x{:03X}", 0x100 + i, 0x110 + i);
        let expect_nospace = format!("MOVFF 0x{:03X},0x{:03X}", 0x100 + i, 0x110 + i);
        assert!(
            asm.contains(&expect) || asm.contains(&expect_nospace),
            "byte {i} missing:\n{asm}"
        );
    }
}

#[test]
fn a_long_consecutive_copy_run_becomes_a_postinc_loop() {
    // Twelve consecutive src+i -> dst+i pairs cost 2 words each
    // straight-line; past COPY_LOOP_MIN_PAIRS the drain replaces the run
    // with one LFSR-seeded POSTINC loop whose count lives in WREG
    // (0xFE8, a file register the ISR save area covers) (epic-cc#486).
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 12\n\
             ret void\n",
    );
    let addrs = addrs(&[("src", 0x100), ("dst", 0x110)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("LFSR 0, 0x100"), "source seed missing:\n{asm}");
    assert!(
        asm.contains("LFSR 1, 0x110"),
        "destination seed missing:\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0x0C"),
        "the 12-byte count is missing:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFEE, 0xFE6"),
        "the POSTINC0 -> POSTINC1 body is missing:\n{asm}"
    );
    assert!(
        asm.contains("DECFSZ 0xFE8,F,A"),
        "the WREG-counted loop tail is missing:\n{asm}"
    );
    assert!(
        !(asm.contains("MOVFF 0x100, 0x110") || asm.contains("MOVFF 0x100,0x110")),
        "the loop replaces the run, straight copies must not coexist:\n{asm}"
    );
}

#[test]
fn a_short_copy_run_stays_straight_line() {
    // Below COPY_LOOP_MIN_PAIRS the 9-word loop loses to the straight
    // 2-words-per-byte form: a 4-byte copy stays four MOVFFs.
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 4\n\
             ret void\n",
    );
    let addrs = addrs(&[("src", 0x100), ("dst", 0x110)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    for i in 0..4u16 {
        let expect = format!("MOVFF 0x{:03X}, 0x{:03X}", 0x100 + i, 0x110 + i);
        let expect_nospace = format!("MOVFF 0x{:03X},0x{:03X}", 0x100 + i, 0x110 + i);
        assert!(
            asm.contains(&expect) || asm.contains(&expect_nospace),
            "byte {i} missing:\n{asm}"
        );
    }
    assert!(
        !asm.contains("MOVFF 0xFEE, 0xFE6"),
        "no POSTINC loop below the threshold:\n{asm}"
    );
}

#[test]
fn a_five_byte_copy_run_stays_straight_line() {
    // At 5 bytes the loop would win one word (9 vs 10) while running
    // roughly 3x slower per byte, so the floor keeps it straight:
    // five MOVFFs, no POSTINC loop (epic-cc#577).
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 5\n\
             ret void\n",
    );
    let addrs = addrs(&[("src", 0x100), ("dst", 0x110)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    for i in 0..5u16 {
        let expect = format!("MOVFF 0x{:03X}, 0x{:03X}", 0x100 + i, 0x110 + i);
        let expect_nospace = format!("MOVFF 0x{:03X},0x{:03X}", 0x100 + i, 0x110 + i);
        assert!(
            asm.contains(&expect) || asm.contains(&expect_nospace),
            "byte {i} missing:\n{asm}"
        );
    }
    assert!(
        !asm.contains("MOVFF 0xFEE, 0xFE6"),
        "no POSTINC loop at the 5-byte floor:\n{asm}"
    );
}

#[test]
fn a_six_byte_copy_run_becomes_a_postinc_loop() {
    // At 6 bytes the loop wins three words (9 vs 12): the smallest run
    // the drain loops (epic-cc#577).
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 6\n\
             ret void\n",
    );
    let addrs = addrs(&[("src", 0x100), ("dst", 0x110)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("LFSR 0, 0x100"), "source seed missing:\n{asm}");
    assert!(
        asm.contains("LFSR 1, 0x110"),
        "destination seed missing:\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0x06"),
        "the 6-byte count is missing:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFEE, 0xFE6"),
        "the POSTINC0 -> POSTINC1 body is missing:\n{asm}"
    );
    assert!(
        !(asm.contains("MOVFF 0x100, 0x110") || asm.contains("MOVFF 0x100,0x110")),
        "the loop replaces the run, straight copies must not coexist:\n{asm}"
    );
}

#[test]
fn a_loop_runs_even_with_an_isr_reachable_fsr1_seeder() {
    // #486 gated the copy loop on "no ISR-reachable function seeds FSR1",
    // because FSR1 was outside the ISR save area (ADR-013). epic-cc#477
    // added FSR1L/FSR1H to every ISR prologue/epilogue, so the loop holds
    // FSR1 safely; the gate is gone and the loop must now run regardless
    // of what the ISR does. (epic-cc#493)
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global arr i8\n\
         global idx i8\n\
         fn feeder(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %p = gep @arr +0 +1*%i\n\
             memcpy @dst %p 8\n\
             ret void\n\
         fn tick(void) [isr] ()\n\
           block entry:\n\
             call void @feeder()\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 12\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("arr", 0x120),
        ("idx", 0x130),
        ("feeder::i", 0x131),
        ("feeder::p", 0x132),
    ]);
    let asm = select_isr(&PIC18F4550, &m, &addrs);
    assert!(
        asm.contains("MOVFF 0xFEE, 0xFE6"),
        "the copy loop must run even when an ISR-reachable function seeds \
         FSR1, now that FSR1 is saved:\n{asm}"
    );
    assert!(
        asm.contains("0xFE1") && asm.contains("0xFE2"),
        "the ISR must save and restore FSR1:\n{asm}"
    );
}

#[test]
fn a_run_draining_before_an_fsr0_reuse_forces_a_reseed() {
    // epic-cc#486 review: access through an sret pointer seeds FSR0 and
    // records the tracked position; a staged 12-pair run emits nothing
    // yet; a second access through the same pointer would reuse FSR0
    // (delta 0) except the drain at its own emission moves FSR0
    // wholesale. The setup must drain before the reuse decision, so the
    // second access re-seeds from the slot instead of reading through a
    // stale pointer.
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global out i8\n\
         fn probe(void) (r=sret)\n\
           block entry:\n\
             %a = load i8 %r\n\
             memcpy @dst @src 12\n\
             %b = load i8 %r\n\
             store i8 %b @out\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("out", 0x120),
        ("probe::r", 0x130),
        ("probe::a", 0x132),
        ("probe::b", 0x133),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVFF 0x130, 0xFE9").count(),
        2,
        "the second access through the same pointer must re-seed FSR0 \
         after the loop drained:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFEE, 0xFE6"),
        "the staged run must still lower to the loop:\n{asm}"
    );
}

#[test]
fn chained_maximal_copies_split_into_byte_sized_loops() {
    // irparse bounds one memcpy at 255 bytes. Two adjacent 255-byte
    // copies stage 510 consecutive pairs, but the second copy's arm
    // drains the first 255 before its own setup runs (the
    // drain-before-decision rule), so each half loops with a byte-sized
    // count instead of one straight replay or an out-of-range MOVLW.
    // (epic-cc#486 review, split refined by epic-cc#492.)
    let m = parse(
        "global src i8\n\
         global src2 i8\n\
         global dst i8\n\
         global dst2 i8\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 255\n\
             memcpy @dst2 @src2 255\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("src2", 0x1FF),
        ("dst", 0x400),
        ("dst2", 0x4FF),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLW 0xFF").count(),
        2,
        "each 255-pair half loops with a byte-sized count:\n{asm}"
    );
    assert_eq!(
        asm.matches("MOVFF 0xFEE, 0xFE6").count(),
        2,
        "each half walks POSTINC:\n{asm}"
    );
    assert!(
        !asm.contains("MOVLW 0x1FE"),
        "no out-of-range MOVLW count may be emitted:\n{asm}"
    );
}

#[test]
fn a_255_byte_run_still_loops() {
    // The gate's upper edge: exactly 255 pairs fits the MOVLW literal
    // and must keep the loop (regression for an off-by-one in the
    // n <= 255 bound).
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 255\n\
             ret void\n",
    );
    let addrs = addrs(&[("src", 0x100), ("dst", 0x400)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVLW 0xFF") && asm.contains("MOVFF 0xFEE, 0xFE6"),
        "a 255-pair run keeps the loop:\n{asm}"
    );
}

#[test]
fn a_loop_runs_even_with_an_isr_reachable_slot_seeder() {
    // The #486 guard's other disjunct: an ISR-reachable function seeding
    // FSR1 through an sret/pointer-param slot. With FSR1 saved on every
    // ISR entry (epic-cc#477), this loops too. (epic-cc#493)
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global buf i8\n\
         fn feeder(void) (r=sret)\n\
           block entry:\n\
             memcpy @dst %r 8\n\
             ret void\n\
         fn tick(void) [isr] ()\n\
           block entry:\n\
             call void @feeder(sret @buf)\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 12\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("buf", 0x120),
        ("feeder::r", 0x121),
    ]);
    let asm = select_isr(&PIC18F4550, &m, &addrs);
    assert!(
        asm.contains("MOVFF 0xFEE, 0xFE6"),
        "the copy loop must run even when an ISR-reachable function seeds \
         FSR1 through a slot:\n{asm}"
    );
}

#[test]
fn a_call_splits_staged_runs_at_the_call_boundary() {
    // Copies staged before a CALL must reach the callee in program
    // order: the run drains as a loop before the CALL emits, and the
    // post-call run seeds afresh (epic-cc#486 review).
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global src2 i8\n\
         global dst2 i8\n\
         fn f(void) ()\n\
           block entry:\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 6\n\
             call void @f()\n\
             memcpy @dst2 @src2 6\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("src2", 0x140),
        ("dst2", 0x150),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let call = asm.find("CALL f").expect("the call is present");
    let first_loop_tail = asm.find("BRA tmp0").expect("the first run loops");
    let second_seed = asm.find("LFSR 0, 0x140").expect("the second run loops");
    assert!(
        first_loop_tail < call && call < second_seed,
        "the first run must drain before the CALL and the second must \
         seed after it:\n{asm}"
    );
}

#[test]
fn a_branch_splits_staged_runs_at_the_block_boundary() {
    // Same contract across a conditional branch: the entry block's run
    // drains before the branch is emitted, so it lands on the
    // predecessor side of the target label, and a run staged in the
    // target block drains after it (epic-cc#486 review).
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global src2 i8\n\
         global dst2 i8\n\
         global flag i8\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @dst @src 6\n\
             %c = load i8 @flag\n\
             br i1 %c t f\n\
           block t:\n\
             memcpy @dst2 @src2 6\n\
             ret void\n\
           block f:\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("src2", 0x140),
        ("dst2", 0x150),
        ("flag", 0x180),
        ("main::c", 0x181),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let first_loop_tail = asm.find("BRA tmp0").expect("the entry run loops");
    let target_label = asm.find("main_Lt:").expect("the branch target label");
    let last_loop_body = asm.rfind("MOVFF 0xFEE, 0xFE6").expect("block t loops");
    assert!(
        first_loop_tail < target_label && target_label < last_loop_body,
        "the entry run must drain on the predecessor side of the label \
         and block t's run after it:\n{asm}"
    );
}

#[test]
fn a_memcpy_to_a_dynamic_indexed_destination_writes_through_indf0() {
    // dst behind a dynamic index (`%dp = gep @dst +0 +1*%i`): the
    // destination resolves to FSR0/INDF0, so each copied byte must be
    // MOVFF'd from the direct source address into 0xFEF (INDF0), not
    // into a statically folded address.
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %dp = gep @dst +0 +1*%i\n\
             memcpy %dp @src 1\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("idx", 0x120),
        ("main::i", 0x121),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVFF 0x100, 0xFEF") || asm.contains("MOVFF 0x100,0xFEF"),
        "the copied byte must go from the direct source (0x100) through INDF0 (0xFEF):\n{asm}"
    );
}

#[test]
fn memcpy_with_indirect_source_walks_postinc1() {
    // Source behind a dynamic index resolves to FSR1/INDF1: byte 0
    // seeds FSR1 once and later bytes walk POSTINC1 instead of
    // re-seeding per byte. (epic-cc#492)
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %p = gep @src +0 +1*%i\n\
             memcpy @dst %p 4\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("idx", 0x120),
        ("main::i", 0x121),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("LFSR 1,").count(),
        1,
        "FSR1 must seed exactly once for the whole copy:\n{asm}"
    );
    assert_eq!(
        asm.matches("MOVF 0xFE6,W,A").count(),
        4,
        "all four bytes must advance through POSTINC1 (the banked dst \
         keeps the W-staged form):\n{asm}"
    );
    assert!(
        !asm.contains("0xFE7"),
        "no INDF1 read remains once the walk advances:\n{asm}"
    );
    for i in 0..4u16 {
        let expect = format!("MOVWF 0x{:03X},B", 0x10 + i);
        assert!(
            asm.contains(&expect),
            "walk byte {i} must write 0x{:03X}:\n{asm}",
            0x110 + i
        );
    }
}

#[test]
fn const_memcpy_walks_tblrd_postinc() {
    // Flash source: TBLPTR seeds once and later bytes read TBLRD*+
    // instead of re-seeding per byte. (epic-cc#492)
    let m = with_bytes(
        parse(
            "const t i8\n\
             global dst i8\n\
             fn main(void) ()\n\
               block entry:\n\
                 memcpy @dst @t 4\n\
                 ret void\n",
        ),
        "t",
        &[0x11, 0x22, 0x33, 0x44],
    );
    let asm = select(&PIC18F4550, &m, &addrs(&[("dst", 0x110)]), None);
    let tblrd: Vec<&str> = asm
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("TBLRD"))
        .collect();
    assert_eq!(
        tblrd,
        vec!["TBLRD*+", "TBLRD*+", "TBLRD*+", "TBLRD*+"],
        "every byte advances through post-increment, byte 0 included:\n{asm}"
    );
}

#[test]
fn single_byte_indirect_memcpy_does_not_walk() {
    // One byte has nothing to walk from: byte 0's full path is the
    // whole copy, with no POSTINC form emitted. (epic-cc#492)
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %p = gep @src +0 +1*%i\n\
             memcpy @dst %p 1\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("idx", 0x120),
        ("main::i", 0x121),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("0xFE6"),
        "no POSTINC1 form in a one-byte copy:\n{asm}"
    );
    assert!(
        asm.contains("MOVF 0xFE7,W,A"),
        "the byte still moves through W:\n{asm}"
    );
}

#[test]
fn indirect_memcpy_walks_even_with_an_isr_reachable_fsr1_seeder() {
    // The per-byte fallback existed only because FSR1 was unsaved. Now
    // the indirect-source copy walks POSTINC1 in every module, and the
    // ISR that seeds FSR1 saves and restores it. (epic-cc#493)
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global arr i8\n\
         global idx i8\n\
         fn feeder(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %p = gep @arr +0 +1*%i\n\
             memcpy @dst %p 8\n\
             ret void\n\
         fn tick(void) [isr] ()\n\
           block entry:\n\
             call void @feeder()\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             %j = load i8 @idx\n\
             %q = gep @src +0 +1*%j\n\
             memcpy @dst %q 4\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("arr", 0x120),
        ("idx", 0x130),
        ("feeder::i", 0x131),
        ("main::j", 0x132),
    ]);
    let asm = select_isr(&PIC18F4550, &m, &addrs);
    // main's 4-byte indirect-source copy: byte 0 seeds FSR1 once and the
    // other three walk POSTINC1 into consecutive destination bytes.
    assert!(
        asm.contains("0xFE6"),
        "the indirect-source copy must walk POSTINC1:\n{asm}"
    );
    for i in 0..4u16 {
        let expect = format!("MOVWF 0x{:03X},B", 0x10 + i);
        assert!(
            asm.contains(&expect),
            "walk byte {i} must write 0x{:03X} (each address computed once):\n{asm}",
            0x110 + i
        );
    }
}

#[test]
fn memcpy_to_dynamic_dst_walks_postinc0() {
    // Destination behind a dynamic index resolves to FSR0/INDF0: byte
    // 0 seeds FSR0 once and later bytes walk POSTINC0, re-grounding
    // the tracked position afterwards. (epic-cc#492)
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %dp = gep @dst +0 +1*%i\n\
             memcpy %dp @src 4\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("idx", 0x120),
        ("main::i", 0x121),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    // Counted in main's body only: `__start`'s zero-clear (epic-cc#561)
    // also walks POSTINC0 (CLRF 0xFEE,A) while wiping the globals.
    let main_asm = asm.split("__start:").next().unwrap();
    assert_eq!(
        main_asm.matches("LFSR 0,").count(),
        1,
        "FSR0 must seed exactly once for the whole copy:\n{asm}"
    );
    assert_eq!(
        main_asm.matches(", 0xFEE").count(),
        3,
        "bytes 0-2 must walk POSTINC0 from the direct src:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0x103, 0xFEF"),
        "the last byte must close with INDF0, leaving FSR0 on it:\n{asm}"
    );
}

#[test]
fn indirect_to_indirect_walk_covers_the_n2_edge() {
    // No middle bytes at n=2: byte 0 advances both pointers, the last
    // byte advances the source and closes the destination with INDF0.
    // (epic-cc#492 review)
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %p = gep @src +0 +1*%i\n\
             %dp = gep @dst +0 +1*%i\n\
             memcpy %dp %p 2\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("idx", 0x120),
        ("main::i", 0x121),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("LFSR 1,").count(),
        1,
        "FSR1 must seed exactly once:\n{asm}"
    );
    assert_eq!(
        // Counted in main's body only: `__start`'s zero-clear
        // (epic-cc#561) contributes its own LFSR 0 loop.
        asm.split("__start:")
            .next()
            .unwrap()
            .matches("LFSR 0,")
            .count(),
        1,
        "FSR0 must seed exactly once:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFE6, 0xFEE"),
        "byte 0 must advance both pointers:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFE6, 0xFEF"),
        "the last byte must advance the source and close INDF0:\n{asm}"
    );
}

#[test]
fn const_to_indirect_walk_covers_the_n2_edge() {
    // Flash source into a dynamic destination at n=2: TBLRD*+ on both
    // bytes, POSTINC0 then INDF0 on the destination side.
    // (epic-cc#492 review)
    let m = with_bytes(
        parse(
            "const t i8\n\
             global dst i8\n\
             global idx i8\n\
             fn main(void) ()\n\
               block entry:\n\
                 %i = load i8 @idx\n\
                 %dp = gep @dst +0 +1*%i\n\
                 memcpy %dp @t 2\n\
                 ret void\n",
        ),
        "t",
        &[0x11, 0x22],
    );
    let addrs = addrs(&[("dst", 0x110), ("idx", 0x120), ("main::i", 0x121)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("TBLRD*+").count(),
        2,
        "both bytes must advance TBLPTR:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFF5, 0xFEE"),
        "byte 0 must advance the destination:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0xFF5, 0xFEF"),
        "the last byte must close INDF0:\n{asm}"
    );
}

#[test]
fn indirect_source_walk_covers_the_n2_edge() {
    // Indirect source into a direct destination at n=2: both bytes read
    // POSTINC1 through W. (epic-cc#492 review)
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %p = gep @src +0 +1*%i\n\
             memcpy @dst %p 2\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("idx", 0x120),
        ("main::i", 0x121),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVF 0xFE6,W,A").count(),
        2,
        "both bytes must advance through POSTINC1:\n{asm}"
    );
    assert!(!asm.contains("0xFE7"), "no INDF1 read remains:\n{asm}");
}

#[test]
fn staged_run_drains_before_a_memcpy_setup() {
    // A direct copy stages a loop-sized run; the next copy's arm must
    // drain it before byte 0's own FSR0 setup runs, so the seed cannot
    // read a stale tracked position. (epic-cc#486 review rule)
    let m = parse(
        "global s1 i8\n\
         global d1 i8\n\
         global s2 i8\n\
         global d2 i8\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             memcpy @d1 @s1 8\n\
             %i = load i8 @idx\n\
             %dp = gep @d2 +0 +1*%i\n\
             memcpy %dp @s2 4\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("s1", 0x100),
        ("d1", 0x110),
        ("s2", 0x140),
        ("d2", 0x150),
        ("idx", 0x160),
        ("main::i", 0x161),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let loop_tail = asm.find("BRA tmp0").expect("the staged run loops");
    let seed = asm.find("LFSR 0, 0x150").expect("the walk seeds FSR0");
    assert!(
        loop_tail < seed,
        "the staged run must drain before the next copy's seed:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "not yet supported")]
fn a_dynamic_length_memcpy_panics_loudly() {
    // A runtime length would need a loop; P3's scope is constant-length
    // memcpy only, so a `MemLen::Reg` must panic loudly rather than
    // silently copy a wrong number of bytes.
    let m = parse(
        "global src i8\n\
         global dst i8\n\
         global n i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %len = load i8 @n\n\
             memcpy @dst @src %len\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("src", 0x100),
        ("dst", 0x110),
        ("n", 0x120),
        ("main::len", 0x121),
    ]);
    select(&PIC18F4550, &m, &addrs, None);
}

#[test]
fn alloca_and_gep_emit_nothing_of_their_own() {
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             %buf = alloca 4\n\
             %p = gep %buf +1\n\
             store i8 9 %p\n\
             ret void\n",
    );
    let addrs = addrs(&[("main::buf", 0x110)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    // The store must land at buf+1 (0x111): proof both Alloca and Gep
    // were handled (the seed + the fold), not merely "didn't crash."
    // 0x111 is bank 1, f=0x11, so the banked emission is MOVLB 0x1 +
    // MOVWF 0x011,B (the literal "0x111" never appears in the asm).
    assert!(
        asm.contains("MOVLB 0x1") && asm.contains("MOVWF 0x011,B"),
        "store must target buf+1 (0x111):\n{asm}"
    );
}

#[test]
fn const_byte_load_emits_tblrd() {
    let m = with_bytes(
        parse("const t i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @t\n    ret void\n"),
        "t",
        &[0x2A],
    );
    let addrs = addrs(&[("main::1", 0x10)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("TBLRD*"),
        "a const read must use TBLRD:\n{asm}"
    );
    // TBLPTRL is SFR 0xFF6, addressed as register-file byte 0xF6; the
    // table base's low byte must seed it before the read.
    assert!(
        asm.contains("MOVWF 0xF6,A"),
        "the table base must seed TBLPTR:\n{asm}"
    );
    // TABLAT is SFR 0xFF5; the read result must be copied from it into the
    // dst slot (the instruction itself never spells the mnemonic "TABLAT").
    assert!(
        asm.contains("MOVFF 0xFF5, 0x010"),
        "TABLAT must be copied into the dst slot:\n{asm}"
    );
}

#[test]
fn const_dynamic_index_load_uses_tblptr_add() {
    // A register-indexed const read: TBLPTR = base + k + scale*%reg, then
    // TBLRD*. The dynamic term must be ADDed onto the seeded TBLPTR with
    // carry, not silently dropped. (The canonical IR text carries no
    // initializer bytes: those arrive from irparse's .ll decode, so the
    // GEP/base shape is what this test pins.)
    let m = with_bytes(
        parse(
            "const t i8\n\
             global in i8\n\
             fn main(void) ()\n\
               block entry:\n\
                 %1 = load i8 @in\n\
                 %p = gep @t +0 +1*%1\n\
                 %2 = load i8 %p\n\
                 ret void\n",
        ),
        "t",
        &[10, 20, 30, 40],
    );
    let addrs = addrs(&[("in", 0x10), ("main::1", 0x11), ("main::2", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("TBLRD*"), "const read must use TBLRD:\n{asm}");
    assert!(
        asm.contains("ADDWF 0xF6,F,A"),
        "index must add onto TBLPTRL:\n{asm}"
    );
    assert!(
        asm.contains("ADDWFC 0xF7,F,A"),
        "carry must propagate into TBLPTRH:\n{asm}"
    );
    assert!(
        asm.contains("ADDWFC 0xF8,F,A"),
        "carry must propagate into TBLPTRU:\n{asm}"
    );
}

#[test]
fn a_large_stride_gep_scales_through_mulwf() {
    // A 12-byte struct stride makes the naive per-add loop (12 x 4
    // words) cost more than the zero-seeded shift-add chain: seed
    // LFSR 0 with 0, one index add, three doublings (12 = 1100b), one
    // conditional add, then the base re-joining as a 16-bit literal
    // add. FSR0 must end at base + 12*i, the same value the unrolled
    // adds produce.
    let m = parse(
        "global recs i8\n\
         global idx i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i8 @idx\n\
             %p = gep @recs +0 +12*%i\n\
             %v = load i8 %p\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("recs", 0x120),
        ("idx", 0x150),
        ("main::i", 0x151),
        ("main::v", 0x152),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    // A 12-byte stride now takes the MULWF form (6 words flat) rather
    // than the shift-add chain (17+). (epic-cc#477)
    assert!(
        asm.contains("MOVLW 0x0C") && asm.contains("MULWF"),
        "stride 12 must scale through MULWF:\n{asm}"
    );
    // The base (0x120) still seeds FSR0, and the product folds onto the
    // pair in access mode (operand() sees 0xFE9/0xFEA as SFR addresses).
    assert!(
        asm.contains("LFSR 0, 0x120") || asm.contains("LFSR 0,0x120"),
        "the base must seed FSR0:\n{asm}"
    );
    assert!(
        !asm.contains("ADDWF 0x0E9,F,B") && !asm.contains("ADDWFC 0x0EA,F,B"),
        "FSR pair updates must stay access-mode:\n{asm}"
    );
}

#[test]
fn a_width2_index_folds_its_high_byte_through_mulwf() {
    // A 12-byte stride with a 16-bit index (clang zero-extends every GEP
    // index, so width 2 is the real frontend shape): the chain's index
    // adds must read the index's HIGH byte where the width-1 chain emits
    // its carry-fill MOVLW, at the identical word cost. Same shape as
    // the width-1 test: seed, 3 doublings (12 = 1100b), 2 chain index
    // adds plus the base re-add.
    let m = parse(
        "global recs i8\n\
         global idx i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i16 @idx\n\
             %p = gep @recs +0 +12*%i\n\
             %v = load i8 %p\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("recs", 0x120),
        ("idx", 0x150),
        ("main::i", 0x152),
        ("main::v", 0x154),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    // Stride 12 takes MULWF; a 16-bit index costs a second MULWF on the
    // high byte (so the whole stride is exact). (epic-cc#477)
    assert!(
        asm.contains("MULWF"),
        "stride 12 must scale through MULWF:\n{asm}"
    );
    // The wide fold: the index's high byte (slot 0x152 + 1, banked so
    // operand() spells 0x053) must be read for its own product. The
    // width-1 form has no second MULWF.
    assert!(
        asm.contains("MULWF 0x053") || asm.contains("MULWF 0x53"),
        "the index high byte must reach the multiplier:\n{asm}"
    );
}

#[test]
fn a_width2_chain_index_reads_the_high_byte_in_sim() {
    // idx = 0x0114 (276): scale 5 gives 0x564, so recs (0x120) + 0x564
    // = 0x684. A high-byte-dropping term would read 0x120 + 5*0x14 =
    // 0x184 instead. Stride 5 keeps the chain gate satisfied (14 + 2 <=
    // 20 naive words) while the addressed byte stays inside the 4550's
    // 2 KiB RAM, which no stride-12 high byte allows.
    let m = parse(
        "global recs i8\n\
         global idx i16\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i16 @idx\n\
             %p = gep @recs +0 +5*%i\n\
             %v = load i8 %p\n\
             store i8 %v @out\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("recs", 0x120),
        ("idx", 0x150),
        ("out", 0x152),
        ("main::i", 0x153),
        ("main::v", 0x155),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MULWF"),
        "stride 5 must scale through MULWF, not the naive loop:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x150] = 0x14; // idx lo byte
    p.ram_mut()[0x151] = 0x01; // idx hi byte: 0x0114 = 276
    p.ram_mut()[0x684] = 0x5A; // the byte the address math must land on
    p.ram_mut()[0x184] = 0xA5; // the byte a low-byte-only term reads
    p.run(500);
    assert_eq!(p.ram()[0x152], 0x5A, "out must be recs[5*0x0114]:\n");
}

#[test]
fn a_width2_small_scale_index_adds_the_high_byte_in_sim() {
    // Stride 2 stays below the chain gate (naive 8 words vs chain 7 + 2
    // overhead), so every unrolled repetition must fold idx_hi itself:
    // idx = 0x0130 scales to 0x260, recs (0x120) + 0x260 = 0x380, while
    // a low-byte-only term lands at 0x120 + 2*0x30 = 0x180.
    let m = parse(
        "global recs i8\n\
         global idx i16\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = load i16 @idx\n\
             %p = gep @recs +0 +2*%i\n\
             %v = load i8 %p\n\
             store i8 %v @out\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("recs", 0x120),
        ("idx", 0x150),
        ("out", 0x152),
        ("main::i", 0x153),
        ("main::v", 0x155),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("LFSR 0, 0x000") && !asm.contains("LFSR 0,0x000"),
        "stride 2 must keep the naive unrolled adds:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x150] = 0x30; // idx lo byte
    p.ram_mut()[0x151] = 0x01; // idx hi byte: 0x0130 = 304
    p.ram_mut()[0x380] = 0x3C; // the byte the address math must land on
    p.ram_mut()[0x180] = 0xC3; // the byte a low-byte-only term reads
    p.run(500);
    assert_eq!(p.ram()[0x152], 0x3C, "out must be recs[2*0x0130]:\n");
}

#[test]
fn a_call_defined_index_width_folds_the_high_byte() {
    // The real frontend shape for a float-derived index: legalize
    // rewrites `(int)f` into a `__fptosi_f32` call, so the index
    // register is defined by a Call, not a FloatConv. Its width rides
    // the call's type, and the naive term must fold the high byte.
    // (epic-cc#488)
    let m = parse(
        "global recs i8\n\
         global fv float\n\
         global out i8\n\
         fn __fptosi_f32(i16) (val=i32)\n\
           block entry:\n\
             %__scr = alloca 8\n\
         fn main(void) ()\n\
           block entry:\n\
             %f = load float @fv\n\
             %i = call i16 @__fptosi_f32(float %f)\n\
             %p = gep @recs +0 +2*%i\n\
             %v = load i8 %p\n\
             store i8 %v @out\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("recs", 0x120),
        ("fv", 0x150),
        ("out", 0x154),
        ("main::f", 0x156),
        ("main::i", 0x15A),
        ("main::v", 0x15C),
        ("__fptosi_f32::val", 0x160),
        ("__fptosi_f32::__scr", 0x164),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("LFSR 0, 0x000") && !asm.contains("LFSR 0,0x000"),
        "stride 2 keeps the naive unrolled adds:\n{asm}"
    );
    // The index slot 0x15A is banked, so the high byte reads as 0x05B.
    assert_eq!(
        asm.matches("MOVF 0x05B").count(),
        2,
        "both unrolled adds must fold the index high byte:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    // 304.0f = 0x43980000 at @fv: the routine converts it to index
    // 0x0130, so the high byte is nonzero and a width-1 fold lands
    // elsewhere.
    for (i, b) in 304.0f32.to_le_bytes().iter().enumerate() {
        p.ram_mut()[0x150 + i] = *b;
    }
    p.ram_mut()[0x380] = 0x6E; // recs + 2*0x0130
    p.ram_mut()[0x180] = 0x91; // the byte a dropped high byte would read
    p.run(100_000);
    assert_eq!(
        p.ram()[0x154],
        0x6E,
        "out must be recs[2*0x0130], not the low-byte-only address"
    );
}

#[test]
fn a_va_arg_defined_index_width_folds_the_high_byte() {
    // The one width arm among the ticket's four that is reachable in the
    // driver pipeline: a `va_arg i16` result used as a GEP index folds
    // its high byte. `fptosi`/`fcmp` results arrive as Call/i1 instead.
    // (epic-cc#488)
    let m = parse(
        "global recs i8\n\
         global out i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %i = va_arg %list i16\n\
             %p = gep @recs +0 +2*%i\n\
             %v = load i8 %p\n\
             store i8 %v @out\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("recs", 0x120),
        ("out", 0x154),
        ("main::list", 0x150),
        ("main::i", 0x158),
        ("main::v", 0x15A),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    // The index slot 0x158 is banked, so the high byte reads as 0x059.
    assert_eq!(
        asm.matches("MOVF 0x059").count(),
        2,
        "both unrolled adds must fold the va_arg index high byte:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    // The va_list slot holds a runtime pointer the read walks: point it
    // at 0x200, where the i16 argument 0x0130 sits.
    p.ram_mut()[0x150] = 0x00;
    p.ram_mut()[0x151] = 0x02;
    p.ram_mut()[0x200] = 0x30;
    p.ram_mut()[0x201] = 0x01;
    p.ram_mut()[0x380] = 0x7D; // recs + 2*0x0130
    p.ram_mut()[0x180] = 0xD7; // the byte a dropped high byte would read
    p.run(100_000);
    assert_eq!(
        p.ram()[0x154],
        0x7D,
        "out must be recs[2*0x0130], not the low-byte-only address"
    );
}

#[test]
fn a_width2_const_index_chains_on_tblptr() {
    // A 12-byte-stride const read with a 16-bit runtime index: the naive
    // TBLPTR loop would cost 72 words; the zero-seeded TBLPTR chain (33)
    // wins. The static MOVLW/MOVWF seeding is replaced by CLRFs and the
    // table base re-joins as literal adds after the chain.
    let m = with_bytes(
        parse(
            "const tab i8\n\
             global idx i16\n\
             fn main(void) ()\n\
               block entry:\n\
                 %i = load i16 @idx\n\
                 %p = gep @tab +0 +12*%i\n\
                 %v = load i8 %p\n\
                 ret void\n",
        ),
        "tab",
        &[0],
    );
    let addrs = addrs(&[("idx", 0x150), ("main::i", 0x152), ("main::v", 0x154)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("MOVWF 0xF6"),
        "the chain must zero-seed TBLPTR, not statically seed it:\n{asm}"
    );
    assert!(
        asm.contains("CLRF 0xF6,A") && asm.contains("CLRF 0xF7,A") && asm.contains("CLRF 0xF8,A"),
        "the chain must zero-seed the TBLPTR triple:\n{asm}"
    );
    let rlcf_tblptrl = asm.matches("RLCF 0xF6").count();
    assert_eq!(
        rlcf_tblptrl, 3,
        "three doublings for a 4-bit stride:\n{asm}"
    );
    let addwf_tblptrl = asm.matches("ADDWF 0xF6").count();
    assert_eq!(
        addwf_tblptrl, 3,
        "two chain index adds plus the base re-add:\n{asm}"
    );
    let movf_idx_hi = asm.matches("MOVF 0x053").count();
    assert_eq!(movf_idx_hi, 2, "both chain adds must read idx_hi:\n{asm}");
    assert!(
        asm.contains("MOVLW LOW(tab)") && asm.contains("ADDWFC 0xF8"),
        "the table base must re-join after the chain, carries reaching TBLPTRU:\n{asm}"
    );
}

#[test]
fn a_width2_const_index_stays_naive_below_the_gate() {
    // Stride 2 on the 3-byte accumulator: naive 12 words vs chain
    // 19 + 2 overhead, so the unrolled loop stays and folds idx_hi per
    // repetition.
    let m = with_bytes(
        parse(
            "const tab i8\n\
             global idx i16\n\
             fn main(void) ()\n\
               block entry:\n\
                 %i = load i16 @idx\n\
                 %p = gep @tab +0 +2*%i\n\
                 %v = load i8 %p\n\
                 ret void\n",
        ),
        "tab",
        &[0],
    );
    let addrs = addrs(&[("idx", 0x150), ("main::i", 0x152), ("main::v", 0x154)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("CLRF 0xF6,A"),
        "stride 2 must keep the naive TBLPTR loop:\n{asm}"
    );
    let addwf_tblptrl = asm.matches("ADDWF 0xF6").count();
    assert_eq!(addwf_tblptrl, 2, "two unrolled repetitions:\n{asm}");
    let movf_idx_hi = asm.matches("MOVF 0x053").count();
    assert_eq!(movf_idx_hi, 2, "both repetitions must read idx_hi:\n{asm}");
}

#[test]
fn a_width1_const_index_chains_on_tblptr_in_sim() {
    // The width-1 variant of the TBLPTR chain, reachable from in-tree IR
    // text (only real clang output always zexts GEP indices to i16): an
    // i8 index over stride 6 clears the 3-byte gate (27 + 2 <= 30 naive
    // words), so the zero-seeded chain must scale idx = 200 to byte
    // offset 1200 exactly.
    let mut bytes = vec![0u8; 1201];
    bytes[1200] = 0x96;
    bytes[6 * 199] = 0x69;
    let m = with_bytes(
        parse(
            "const tab i8\n\
             global idx i8\n\
             global out i8\n\
             fn main(void) ()\n\
               block entry:\n\
                 %i = load i8 @idx\n\
                 %p = gep @tab +0 +6*%i\n\
                 %v = load i8 %p\n\
                 store i8 %v @out\n\
                 ret void\n",
        ),
        "tab",
        &bytes,
    );
    let addrs = addrs(&[
        ("idx", 0x150),
        ("out", 0x151),
        ("main::i", 0x152),
        ("main::v", 0x153),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("CLRF 0xF6,A"),
        "stride 6 must take the TBLPTR chain:\n{asm}"
    );
    let hex = asm::assemble_file_to_hex(&PIC18F4550, &asm);
    let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x150] = 200;
    p.run(1000);
    assert_eq!(p.ram()[0x151], 0x96, "out must be tab[6*200]:\n");
}

#[test]
fn a_width2_const_index_reads_the_high_byte_in_sim() {
    // The flash-side miscompile proof: tab is 2323 bytes, idx = 0x0102,
    // scale 9 -> byte offset 0x912 (2322), the table's last byte. A
    // high-byte-dropping term reads tab[9*2] = tab[18] instead. Stride 9
    // clears the TBLPTR chain gate (33 + 2 <= 54 naive words) where
    // stride 5 does not (31 > 30).
    let mut bytes = vec![0u8; 2323];
    bytes[2322] = 0x96;
    bytes[18] = 0x69;
    let m = with_bytes(
        parse(
            "const tab i8\n\
             global idx i16\n\
             global out i8\n\
             fn main(void) ()\n\
               block entry:\n\
                 %i = load i16 @idx\n\
                 %p = gep @tab +0 +9*%i\n\
                 %v = load i8 %p\n\
                 store i8 %v @out\n\
                 ret void\n",
        ),
        "tab",
        &bytes,
    );
    let addrs = addrs(&[
        ("idx", 0x150),
        ("out", 0x152),
        ("main::i", 0x153),
        ("main::v", 0x155),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("CLRF 0xF6,A"),
        "stride 9 must take the TBLPTR chain:\n{asm}"
    );
    let hex = asm::assemble_file_to_hex(&PIC18F4550, &asm);
    let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x150] = 0x02; // idx lo byte
    p.ram_mut()[0x151] = 0x01; // idx hi byte: 0x0102 = 258
    p.run(1000);
    assert_eq!(p.ram()[0x152], 0x96, "out must be tab[9*0x0102]:\n");
}

#[test]
fn const_i16_load_reads_two_bytes() {
    let m = with_bytes(
        parse(
            "const t i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @t\n    ret void\n",
        ),
        "t",
        &[0x34, 0x12],
    );
    let addrs = addrs(&[("main::1", 0x10)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    // Two independent TBLRD* reads, into dst byte 0 and byte 1.
    assert_eq!(
        asm.matches("TBLRD*").count(),
        2,
        "two bytes need two TBLRD reads:\n{asm}"
    );
}

#[test]
#[should_panic(expected = "ROM is not writable")]
fn const_store_panics() {
    let m =
        parse("const t i8\nfn main(void) ()\n  block entry:\n    store i8 5 @t\n    ret void\n");
    let _ = select(&PIC18F4550, &m, &HashMap::new(), None);
}

#[test]
#[should_panic(expected = "ROM is not writable")]
fn store_through_const_gep_reg_panics() {
    let m = parse(
        "const t i8\n\
         global in i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @in\n\
             %2 = gep @t +0 +1*%1\n\
             store i8 9 %2\n\
             ret void\n",
    );
    let addrs = addrs(&[("in", 0x10), ("main::1", 0x11)]);
    let _ = select(&PIC18F4550, &m, &addrs, None);
}

#[test]
fn emits_const_tables_as_db_after_start() {
    // The const table's bytes are emitted as `db` lines between __start
    // and `end`, so the TBLRD reads resolve through the label.
    let m = with_bytes(
        ir::parse("const t i8\nfn main(void) ()\n  block entry:\n    ret void\n"),
        "t",
        &[0x0A, 0x14, 0x1E, 0x28],
    );
    let asm = select(&PIC18F4550, &m, &addrs(&[]), None);
    assert!(asm.contains("t:"), "table label must be emitted:\n{asm}");
    assert!(
        asm.contains("db 0x0A, 0x14, 0x1E, 0x28"),
        "the four bytes must be on one db line:\n{asm}"
    );
    assert!(
        asm.rfind("t:")
            .map(|i| asm[i..].contains("db"))
            .unwrap_or(false),
        "the db bytes must come after the label:\n{asm}"
    );
}

#[test]
fn const_table_ram_ref_materializes_the_alloc_address() {
    // A `static const` struct field initialized with a RAM global's
    // ADDRESS (epic-hal's combo-modbus register-map shape): the refs
    // name a RAM global, which has no assembler label. Before #443 the
    // table emitted `db LOW(holding_regs)` and the assembler panicked.
    // holding_regs sits at 0x210: LOW = 0x10, HIGH = 0x02.
    let m = with_refs(
        with_bytes(
            parse("const map i8\nfn main(void) ()\n  block entry:\n    ret void\n"),
            "map",
            &[0x00, 0x00, 0x04],
        ),
        "map",
        &[(0, "holding_regs"), (1, "holding_regs")],
    );
    let asm = select(&PIC18F4550, &m, &addrs(&[("holding_regs", 0x210)]), None);
    assert!(
        asm.contains("db 0x10\n    db 0x02"),
        "each ref byte must materialize its address half:\n{asm}"
    );
    assert!(
        !asm.contains("LOW(holding_regs)"),
        "a RAM global has no label to resolve:\n{asm}"
    );
    // Pass 2 resolving every operand is the exact stage #443 panicked in.
    asm::assemble_pic18(&asm);
}

#[test]
fn const_table_function_ref_keeps_the_label_literal() {
    // A function-address field is a link-time value: it must stay a
    // label literal the assembler resolves (epic-cc#154). #443's
    // alloc-address path must not capture it: a function is never in
    // the address map.
    let m = with_refs(
        with_bytes(
            parse(
                "const vt i8\n\
                 fn f0(void) ()\n  block entry:\n    ret void\n\
                 fn main(void) ()\n  block entry:\n    ret void\n",
            ),
            "vt",
            &[0x00, 0x00],
        ),
        "vt",
        &[(0, "f0"), (1, "f0")],
    );
    let asm = select(&PIC18F4550, &m, &addrs(&[]), None);
    assert!(
        asm.contains("db LOW(f0)\n    db HIGH(f0)"),
        "a function address stays a link-time label:\n{asm}"
    );
    asm::assemble_pic18(&asm);
}

#[test]
fn const_to_ram_init_ram_ref_materializes_the_alloc_address() {
    // A const global copied to RAM whose ref names a RAM global: the
    // __start init must write the address bytes numerically (epic-cc#443),
    // not a `MOVLW LOW(...)` label the assembler cannot resolve for a RAM
    // global. cfg at 0x040 (access bank), arr at 0x210.
    let m = with_refs(
        with_bytes(
            parse("const cfg i8\nfn main(void) ()\n  block entry:\n    ret void\n"),
            "cfg",
            &[0x00, 0x00],
        ),
        "cfg",
        &[(0, "arr"), (1, "arr")],
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[("cfg", 0x040), ("arr", 0x210)]),
        None,
    );
    assert!(
        asm.contains("MOVLW 0x10\n    MOVWF 0x040,A\n    MOVLW 0x02\n    MOVWF 0x041,A"),
        "init must write the alloc-address bytes into the RAM copy:\n{asm}"
    );
    assert!(
        !asm.contains("LOW(arr)"),
        "a RAM global has no label to resolve:\n{asm}"
    );
}
// P7 float tests: bit-exact sim per recipe (add, mul, div, cmp, conversions, RNE)
fn f32_le(x: f32) -> [u8; 4] {
    x.to_bits().to_le_bytes()
}
fn float_routine_sig(name: &str) -> (&'static str, &'static [(&'static str, &'static str)], u16) {
    match name {
        "__add_f32" | "__sub_f32" | "__mul_f32" => ("float", &[("a", "i32"), ("b", "i32")], 14),
        "__div_f32" => ("float", &[("a", "i32"), ("b", "i32")], 12),
        "__cmp_f32" => ("i8", &[("a", "i32"), ("b", "i32")], 6),
        "__uitofp_f32" | "__sitofp_f32" => ("float", &[("val", "i32")], 8),
        "__fptoui_f32" | "__fptosi_f32" => ("i32", &[("val", "i32")], 8),
        other => panic!("unknown float routine {other}"),
    }
}
fn float_routine_module(name: &str) -> (String, Vec<(String, u16)>) {
    let (ret, params, scr) = float_routine_sig(name);
    let pstr = params
        .iter()
        .map(|(n, t)| format!("{n}={t}"))
        .collect::<Vec<_>>()
        .join(", ");
    let ir = format!(
        "global ina float\n\
         global inb float\n\
         global out {ret}\n\
         fn {name}({ret}) ({pstr})\n\
           block entry:\n\
             %__scr = alloca {scr}\n\
         fn main(void) ()\n\
           block entry:\n\
             %x = load float @ina\n\
             %y = load float @inb\n\
             %r = call {ret} @{name}(float %x, float %y)\n\
             store {ret} %r @out\n\
             ret void\n"
    );
    let mut map = vec![
        ("ina".to_string(), 0x20u16),
        ("inb".to_string(), 0x24),
        ("out".to_string(), 0x28),
        ("main::x".to_string(), 0x2C),
        ("main::y".to_string(), 0x30),
        ("main::r".to_string(), 0x34),
    ];
    let mut base = 0x40u16;
    for (pn, _) in params {
        map.push((format!("{name}::{pn}"), base));
        base += 4;
    }
    map.push((format!("{name}::__scr"), base));
    (ir, map)
}
fn float_routine_unary_module(name: &str) -> (String, Vec<(String, u16)>) {
    let (ret, params, scr) = float_routine_sig(name);
    let pstr = params
        .iter()
        .map(|(n, t)| format!("{n}={t}"))
        .collect::<Vec<_>>()
        .join(", ");
    let ir = format!(
        "global inv i32\n\
         global out {ret}\n\
         fn {name}({ret}) ({pstr})\n\
           block entry:\n\
             %__scr = alloca {scr}\n\
         fn main(void) ()\n\
           block entry:\n\
             %v = load i32 @inv\n\
             %r = call {ret} @{name}(i32 %v)\n\
             store {ret} %r @out\n\
             ret void\n"
    );
    let map = vec![
        ("inv".to_string(), 0x20u16),
        ("out".to_string(), 0x24),
        ("main::v".to_string(), 0x2C),
        ("main::r".to_string(), 0x30),
        (format!("{name}::val"), 0x40),
        (format!("{name}::__scr"), 0x44),
    ];
    (ir, map)
}
fn sim_run_bytes(
    ir_text: &str,
    map: &[(String, u16)],
    seed: &[(u16, u8)],
    out: u16,
    n: usize,
) -> Vec<u8> {
    let m = ir::parse(ir_text);
    let addrs: HashMap<String, u16> = map.iter().cloned().collect();
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    for &(addr, val) in seed {
        p.ram_mut()[addr as usize] = val;
    }
    p.run(500000);
    (0..n).map(|i| p.ram()[(out + i as u16) as usize]).collect()
}
#[test]
fn float_add_1_plus_1_is_2() {
    let (ir, map) = float_routine_module("__add_f32");
    let mut seed = Vec::new();
    for (i, b) in f32_le(1.0).iter().enumerate() {
        seed.push((0x20 + i as u16, *b));
    }
    for (i, b) in f32_le(1.0).iter().enumerate() {
        seed.push((0x24 + i as u16, *b));
    }
    let got = sim_run_bytes(&ir, &map, &seed, 0x28, 4);
    assert_eq!(got, f32_le(2.0), "1.0+1.0=2.0");
}
#[test]
fn float_sub_1_minus_0_5_is_0_5() {
    let (ir, map) = float_routine_module("__sub_f32");
    let mut seed = Vec::new();
    for (i, b) in f32_le(1.0).iter().enumerate() {
        seed.push((0x20 + i as u16, *b));
    }
    for (i, b) in f32_le(0.5).iter().enumerate() {
        seed.push((0x24 + i as u16, *b));
    }
    let got = sim_run_bytes(&ir, &map, &seed, 0x28, 4);
    assert_eq!(got, f32_le(0.5), "1.0-0.5=0.5");
}
#[test]
fn float_mul_2_times_3_is_6() {
    let (ir, map) = float_routine_module("__mul_f32");
    let mut seed = Vec::new();
    for (i, b) in f32_le(2.0).iter().enumerate() {
        seed.push((0x20 + i as u16, *b));
    }
    for (i, b) in f32_le(3.0).iter().enumerate() {
        seed.push((0x24 + i as u16, *b));
    }
    let got = sim_run_bytes(&ir, &map, &seed, 0x28, 4);
    assert_eq!(got, f32_le(6.0), "2.0*3.0=6.0");
}
#[test]
fn float_div_3_div_2_5_is_1_2() {
    let (ir, map) = float_routine_module("__div_f32");
    let mut seed = Vec::new();
    for (i, b) in f32_le(3.0).iter().enumerate() {
        seed.push((0x20 + i as u16, *b));
    }
    for (i, b) in f32_le(2.5).iter().enumerate() {
        seed.push((0x24 + i as u16, *b));
    }
    let got = sim_run_bytes(&ir, &map, &seed, 0x28, 4);
    assert_eq!(got, f32_le(1.2), "3.0/2.5=1.2 0x3F99999A");
}
#[test]
fn float_div_1_div_3_is_rne() {
    let (ir, map) = float_routine_module("__div_f32");
    let mut seed = Vec::new();
    for (i, b) in f32_le(1.0).iter().enumerate() {
        seed.push((0x20 + i as u16, *b));
    }
    for (i, b) in f32_le(3.0).iter().enumerate() {
        seed.push((0x24 + i as u16, *b));
    }
    let got = sim_run_bytes(&ir, &map, &seed, 0x28, 4);
    assert_eq!(got, f32_le(1.0 / 3.0), "1.0/3.0=0x3EAAAAAB RNE");
}
#[test]
fn float_cmp_tri_state() {
    let cases: &[(&str, f32, f32, u8)] = &[
        ("eq", 1.0, 1.0, 0),
        ("lt", 1.0, 2.0, 1),
        ("gt", 2.0, 1.0, 2),
        ("nan", f32::NAN, 1.0, 3),
    ];
    for &(_, a, b, expect) in cases {
        let (ir, map) = float_routine_module("__cmp_f32");
        let mut seed = Vec::new();
        for (i, by) in f32_le(a).iter().enumerate() {
            seed.push((0x20 + i as u16, *by));
        }
        for (i, by) in f32_le(b).iter().enumerate() {
            seed.push((0x24 + i as u16, *by));
        }
        let got = sim_run_bytes(&ir, &map, &seed, 0x28, 1);
        let val = got[0];
        if a.is_nan() || b.is_nan() {
            assert_eq!(val, 3, "nan cmp should be 3, got {val}");
        } else {
            assert_eq!(val, expect, "cmp {a} vs {b} expected {expect} got {val}");
        }
    }
}
#[test]
fn float_uitofp_9_is_9() {
    let (ir, map) = float_routine_unary_module("__uitofp_f32");
    let mut seed = Vec::new();
    for (i, b) in 9u32.to_le_bytes().iter().enumerate() {
        seed.push((0x20 + i as u16, *b));
    }
    let got = sim_run_bytes(&ir, &map, &seed, 0x24, 4);
    assert_eq!(got, f32_le(9.0), "uitofp 9 -> 9.0");
}
#[test]
fn float_sitofp_minus9_is_minus9() {
    let (ir, map) = float_routine_unary_module("__sitofp_f32");
    let mut seed = Vec::new();
    for (i, b) in (-9i32).to_le_bytes().iter().enumerate() {
        seed.push((0x20 + i as u16, *b));
    }
    let got = sim_run_bytes(&ir, &map, &seed, 0x24, 4);
    assert_eq!(got, f32_le(-9.0), "sitofp -9 -> -9.0");
}
#[test]
fn float_fptosi_truncates() {
    let (ir, map) = float_routine_unary_module("__fptosi_f32");
    let mut seed = Vec::new();
    for (i, b) in f32_le(9.75).iter().enumerate() {
        seed.push((0x20 + i as u16, *b));
    }
    let got = sim_run_bytes(&ir, &map, &seed, 0x24, 4);
    assert_eq!(got, 9i32.to_le_bytes(), "fptosi 9.75 -> 9");
}
#[test]
fn float_fptoui_truncates() {
    let (ir, map) = float_routine_unary_module("__fptoui_f32");
    let mut seed = Vec::new();
    for (i, b) in f32_le(9.75).iter().enumerate() {
        seed.push((0x20 + i as u16, *b));
    }
    let got = sim_run_bytes(&ir, &map, &seed, 0x24, 4);
    assert_eq!(got, 9u32.to_le_bytes(), "fptoui 9.75 -> 9");
}
#[test]
fn float_rne_0_1_plus_0_2() {
    let (ir, map) = float_routine_module("__add_f32");
    let mut seed = Vec::new();
    for (i, b) in f32_le(0.1).iter().enumerate() {
        seed.push((0x20 + i as u16, *b));
    }
    for (i, b) in f32_le(0.2).iter().enumerate() {
        seed.push((0x24 + i as u16, *b));
    }
    let got = sim_run_bytes(&ir, &map, &seed, 0x28, 4);
    assert_eq!(got, f32_le(0.1f32 + 0.2f32), "0.1+0.2 RNE");
}

/// An indirect call through a function pointer lowers to a compare-and-call
/// chain over the candidate set: each candidate's two address bytes are
/// compared against the fp value, the matched arm CALLs, and an unmatched fp
/// falls into a deterministic trap (epic-cc#73).
#[test]
fn indirect_call_emits_compare_and_call_chain() {
    let m = parse(
        "fn f0(void) ()\n  block entry:\n    ret void\n\
         fn f1(void) ()\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n\
           call void %3() callees f0 f1\n    ret void\n",
    );
    let addrs = addrs(&[("main::3", 0x10)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("XORLW LOW(f0)"),
        "compare fp lo against f0:\n{asm}"
    );
    assert!(
        asm.contains("XORLW HIGH(f0)"),
        "compare fp hi against f0:\n{asm}"
    );
    assert!(
        asm.contains("XORLW LOW(f1)"),
        "compare fp against f1:\n{asm}"
    );
    assert!(asm.contains("    CALL f0"), "CALL f0:\n{asm}");
    assert!(asm.contains("    CALL f1"), "CALL f1:\n{asm}");
    assert!(asm.contains("BRA tmp"), "trap loop:\n{asm}");
}

#[test]
fn indirect_candidates_with_unanimous_exit_carry_at_the_join() {
    // Both candidates end on bank 1, so the done label's true entry bank
    // is 1 (only the candidate arms' BRAs reach it; the trap loops). The
    // meet is restored directly after the label: the forward join cannot
    // do it, because the label's linear fall-through comes from the trap
    // block, whose bank is unknown. Without carry this emits a MOVLB in
    // main after the chain; with it, exactly the candidates' two.
    let m = parse(
        "global a i8\nglobal g i8\n\
         fn f0(void) ()\n\
           block entry:\n\
             store i8 1 @h\n\
             ret void\n\
         fn f1(void) ()\n\
           block entry:\n\
             store i8 2 @h\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             call void %3() callees f0 f1\n\
             store i8 9 @g\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("g", 0x110), // bank 1: the post-chain store under test
        ("h", 0x190), // bank 1: both candidates select it and exit on it
        ("main::3", 0x30),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        2,
        "one MOVLB per candidate body, none in main:\n{asm}"
    );
}

#[test]
fn mixed_indirect_candidates_clear_at_the_join() {
    // f1 exits on bank 2, f0 on bank 1: the join must collapse and the
    // post-chain store must re-select bank 1.
    let m = parse(
        "global a i8\nglobal g i8\n\
         fn f0(void) ()\n\
           block entry:\n\
             store i8 1 @h\n\
             ret void\n\
         fn f1(void) ()\n\
           block entry:\n\
             store i8 2 @k\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             call void %3() callees f0 f1\n\
             store i8 9 @g\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("g", 0x110), // bank 1
        ("h", 0x190), // bank 1: f0's exit
        ("k", 0x290), // bank 2: f1's exit
        ("main::3", 0x30),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        3,
        "one MOVLB per candidate body plus main's re-select; nothing carries:\n{asm}"
    );
}

#[test]
fn freeze_copies_bytes_like_a_noop() {
    let m = parse("global a i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = freeze i16 %1\n    ret void\n");
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[("a", 0x10), ("main::1", 0x20), ("main::2", 0x22)]),
        None,
    );
    assert!(
        asm.contains("MOVFF 0x020, 0x022"),
        "freeze copies the value slot:\n{asm}"
    );
}

#[test]
fn inttoptr_copies_the_two_address_bytes() {
    let m = parse("global a i16\nfn main(void) ()\n  block entry:\n    %1 = load i16 @a\n    %2 = inttoptr i16 %1 to ptr\n    ret void\n");
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[("a", 0x10), ("main::1", 0x20), ("main::2", 0x24)]),
        None,
    );
    assert!(
        asm.contains("MOVFF 0x020, 0x024"),
        "inttoptr copies the runtime address bytes:\n{asm}"
    );
}

#[test]
fn inttoptr_const_source_is_rejected_not_silently_miscompiled() {
    let m =
        parse("fn main(void) ()\n  block entry:\n    %1 = inttoptr i16 12 to ptr\n    ret void\n");
    let result =
        std::panic::catch_unwind(|| select(&PIC18F4550, &m, &addrs(&[("main::1", 0x20)]), None));
    assert!(result.is_err(), "const-source IntToPtr must panic loudly");
}

#[test]
fn sext_i1_to_i8_zero_fills_not_sign_fills() {
    let m = parse("global a i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = trunc i8 %1 to i1\n    %3 = sext i1 %2 to i8\n    ret void\n");
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[
            ("a", 0x10),
            ("main::1", 0x20),
            ("main::2", 0x21),
            ("main::3", 0x22),
        ]),
        None,
    );
    assert!(
        asm.contains("MOVFF 0x021, 0x022"),
        "i1 sext of a truncated byte is a same-byte copy:\n{asm}"
    );
}

#[test]
fn sext_i1_to_i16_widening_zero_fills_the_high_byte() {
    // An i1 holds exactly 0/1, so widening zero-fills the high bytes
    // (one CLRF each, not the sign fill): sim-gated over both icmp
    // outcomes, with the CLRF pinned in text.
    let m = parse(
        "global a i8\nglobal b i8\nglobal out i16\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp eq i8 %1, %2\n    %4 = sext i1 %3 to i16\n    store i16 %4 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x10),
        ("b", 0x11),
        ("out", 0x12),
        ("main::1", 0x14),
        ("main::2", 0x15),
        ("main::3", 0x16),
        ("main::4", 0x17),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("CLRF"),
        "i1 widening must clear high bytes with CLRF:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (av, bv, expect_lo) in [(5u8, 5u8, 1u8), (5, 6, 0)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = av;
        p.ram_mut()[0x11] = bv;
        p.run(200);
        assert_eq!(p.ram()[0x12], expect_lo, "sext(icmp eq({av},{bv})) lo");
        assert_eq!(p.ram()[0x13], 0x00, "sext(icmp eq({av},{bv})) hi");
    }
}

#[test]
fn widening_zext_between_compare_and_branch_keeps_the_branch_sound() {
    // A widening cast scheduled between a compare and its consuming
    // branch: the high-byte fill emits CLRF, which sets Z, after the
    // compare's flag-setting sequence and before the branch. The branch
    // stays sound because `BrCond` reloads the condition with `MOVF`
    // (which sets Z fresh) immediately before `BZ`, so it never
    // observes the fill's Z. Each arm stores a distinguishable value
    // so a wrong-target branch fails, not just a non-halting one.
    let m = parse(
        "global a i8\nglobal b i8\nglobal w i8\nglobal out i8\nglobal wide i16\nfn main(void) ()\n  block entry:\n    %1 = load i8 @a\n    %2 = load i8 @b\n    %3 = icmp eq i8 %1, %2\n    %4 = load i8 @w\n    %5 = zext i8 %4 to i16\n    store i16 %5 @wide\n    br i1 %3 t f\n  block t:\n    store i8 1 @out\n    ret void\n  block f:\n    store i8 2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x10),
        ("b", 0x11),
        ("w", 0x12),
        ("out", 0x13),
        ("wide", 0x14),
        ("main::1", 0x16),
        ("main::2", 0x17),
        ("main::3", 0x18),
        ("main::4", 0x19),
        ("main::5", 0x1A),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("CLRF"),
        "the interleaved widening zext must emit a CLRF fill:\n{asm}"
    );
    assert!(
        !asm.contains("MOVLW 0x00"),
        "the widening fill must not keep the stale two-word shape:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (av, bv, expect) in [(5u8, 5u8, 1u8), (5, 6, 2)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = av;
        p.ram_mut()[0x11] = bv;
        p.ram_mut()[0x12] = 0x5A;
        p.run(200);
        assert!(p.halted());
        assert_eq!(
            p.ram()[0x13],
            expect,
            "br on icmp eq({av},{bv}) took the wrong target"
        );
        assert_eq!(p.ram()[0x14], 0x5A, "zext low byte");
        assert_eq!(p.ram()[0x15], 0x00, "zext high byte");
    }
}

/// Priority wiring (epic-cc#346): a high/low pair emits GOTO stubs at
/// both vectors (the bodies cannot share one vector entry), the high
/// ISR on the fixed save block, and the low ISR on its own save area.
#[test]
fn priority_pair_emits_both_vectors_and_save_areas() {
    let m = parse(
        "fn hi(void) [isr] [irq1] ()\n  block entry:\n    ret void\n\
         fn lo(void) [isr] [irq2] ()\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    // A banked save area (0x120, above the access window): W goes
    // through an explicit bank select, not `,A` (which would resolve
    // into the SFR page).
    let asm =
        isel_pic18::select_with_locs(&PIC18F4550, &m, &addrs(&[]), Some(0x120), None, Some(0x160))
            .0;
    assert!(
        asm.contains("org 0x0008") && asm.contains("goto hi"),
        "high stub at vector 0x0008:\n{asm}"
    );
    assert!(
        asm.contains("org 0x0018") && asm.contains("goto lo"),
        "low stub at vector 0x0018:\n{asm}"
    );
    // The high ISR keeps the fixed save block (W at 0x008, a free byte
    // that doesn't collide with FSR0H's slot at 0x004)...
    assert!(
        asm.contains("MOVWF 0x008,A"),
        "high ISR saves W to the fixed block:\n{asm}"
    );
    // ...while the low ISR selects the save bank and addresses W banked.
    assert!(
        asm.contains("MOVLB 0x1") && asm.contains("MOVWF 0x120,B"),
        "low ISR saves W banked:\n{asm}"
    );
    assert!(
        asm.contains("MOVFF 0x000, 0x128"),
        "low ISR snapshots retval into its own area:\n{asm}"
    );
    assert!(
        asm.contains("MOVF 0x120, W, B"),
        "low ISR restores W banked:\n{asm}"
    );
    assert_eq!(
        asm.matches("RETFIE").count(),
        2,
        "both handlers return via RETFIE:\n{asm}"
    );
}

/// one (or a lone ISR with one) is an inconsistent pipeline, not a silent
/// miscompile.
#[test]
#[should_panic(expected = "must be present exactly in priority mode")]
fn priority_pair_without_save_area_panics() {
    let m = parse(
        "fn hi(void) [isr] [irq1] ()\n  block entry:\n    ret void\n\
         fn lo(void) [isr] [irq2] ()\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    let _ = select_isr(&PIC18F4550, &m, &addrs(&[]));
}

#[test]
fn const_zero_store_emits_clrf_without_w_staging() {
    let m = parse(
        "global g i8\nglobal h i16\n\
         fn main(void) ()\n  block entry:\n    store i8 0 @g\n    store i16 0 @h\n    ret void\n",
    );
    let asm = select(&PIC18F4550, &m, &addrs(&[("g", 0x20), ("h", 0x21)]), None);
    assert!(asm.contains("CLRF 0x020,A"), "i8 zero store:\n{asm}");
    assert!(asm.contains("CLRF 0x021,A"), "i16 zero low byte:\n{asm}");
    assert!(asm.contains("CLRF 0x022,A"), "i16 zero high byte:\n{asm}");
    assert_eq!(
        asm.matches("MOVLW 0x00").count(),
        0,
        "zero bytes must not stage through W:\n{asm}"
    );
}

#[test]
fn const_nonzero_store_still_stages_through_w() {
    let m = parse(
        "global g i8\n\
         fn main(void) ()\n  block entry:\n    store i8 7 @g\n    ret void\n",
    );
    let asm = select(&PIC18F4550, &m, &addrs(&[("g", 0x20)]), None);
    assert!(asm.contains("MOVLW 0x07"), "non-zero byte:\n{asm}");
    assert!(asm.contains("MOVWF 0x020,A"), "non-zero byte:\n{asm}");
    assert!(!asm.contains("CLRF 0x020,"), "non-zero byte:\n{asm}");
}

#[test]
fn memcpy_indirect_src_to_banked_direct_dst_selects_the_bank() {
    // A memcpy from an FSR1-indirect source (an sret pointer param) into
    // a banked direct global must select the bank: the stale access-bank
    // spelling reads access-RAM 0x80 instead of bank-1 0x180. The pad
    // globals push `dst` into bank 1 the way a real layout would.
    let mut src = String::new();
    for i in 0..0x70u16 {
        src.push_str(&format!("global pad{i:03} i8\n"));
    }
    src.push_str(
        "global dst i32\n\
         fn f(void) (p=sret)\n\
           block entry:\n\
             memcpy @dst %p 4\n\
             ret void\n",
    );
    let m = parse(&src);
    let mut addrs: HashMap<String, u16> = HashMap::new();
    for i in 0..0x70u16 {
        addrs.insert(format!("pad{i:03}"), 0x10 + i);
    }
    addrs.insert("dst".to_string(), 0x180);
    addrs.insert("f::p".to_string(), 0x300);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVLB 0x1"),
        "the bank-1 destination needs a bank select:\n{asm}"
    );
    assert!(
        asm.contains("MOVWF 0x080,B"),
        "the destination store must be banked:\n{asm}"
    );
    assert!(
        !asm.contains("MOVWF 0x080,A"),
        "the stale access-bank spelling must be gone:\n{asm}"
    );
}

#[test]
fn dense_switch_lowers_to_pcl_table() {
    // epic-cc#479: eight dense cases table through a PCL computed jump:
    // bounds check against n-1, PCLATH named by HIGH(), a `.pcltbl`
    // marker for the assembler's page check, and one absolute GOTO per
    // case.
    let cases = [
        "0 %c0", "1 %c1", "2 %c2", "3 %c3", "4 %c4", "5 %c5", "6 %c6", "7 %c7",
    ];
    let mut blocks = String::new();
    for (i, _) in cases.iter().enumerate() {
        blocks.push_str(&format!(
            "  block c{i}:\n    store i8 {i} @out\n    ret void\n"
        ));
    }
    let m = parse(&format!(
        "global sel i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i16 @sel\n    \
         switch i16 %1, default %def, cases {}\n{blocks}  \
         block def:\n    store i8 99 @out\n    ret void\n",
        cases.join(", ")
    ));
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[("sel", 0x20), ("out", 0x28), ("main::1", 0x30)]),
        None,
    );
    assert!(asm.contains("ADDWF 0xFF9,F,A"), "PCL dispatch:\n{asm}");
    assert!(
        asm.contains("MOVLW HIGH("),
        "table page into PCLATH:\n{asm}"
    );
    assert!(asm.contains(".pcltbl "), "page-check marker:\n{asm}");
    assert!(
        asm.contains("SUBLW 0x07"),
        "bounds check against n-1:\n{asm}"
    );
    assert_eq!(
        asm.matches("\n    GOTO ").count(),
        8,
        "one absolute GOTO per case:\n{asm}"
    );
}

#[test]
fn small_dense_switch_keeps_the_compare_chain() {
    // Three dense cases: below the table crossover, the chain costs
    // less than the dispatch-plus-table shape.
    let m = parse(
        "global sel i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i16 @sel\n    \
         switch i16 %1, default %def, cases 0 %c0, 1 %c1, 2 %c2\n  \
         block c0:\n    store i8 0 @out\n    ret void\n  \
         block c1:\n    store i8 1 @out\n    ret void\n  \
         block c2:\n    store i8 2 @out\n    ret void\n  \
         block def:\n    store i8 99 @out\n    ret void\n",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[("sel", 0x20), ("out", 0x28), ("main::1", 0x30)]),
        None,
    );
    assert!(!asm.contains("ADDWF 0xFF9"), "no PCL dispatch:\n{asm}");
    assert!(!asm.contains(".pcltbl"), "no table marker:\n{asm}");
    assert_eq!(
        // Counted in main's body only: `__start`'s zero-clear
        // (epic-cc#561) contributes its own BRA loop.
        asm.split("__start:")
            .next()
            .unwrap()
            .matches("\n    BRA ")
            .count(),
        4,
        "one branch per case plus the default:\n{asm}"
    );
}

#[test]
fn sparse_switch_keeps_the_compare_chain() {
    // Contiguous-looking but gapped values never table: the table is
    // indexed by the raw value, so gaps would jump wild.
    let m = parse(
        "global sel i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i16 @sel\n    \
         switch i16 %1, default %def, cases 0 %c0, 5 %c1, 9 %c2, 10 %c3, 11 %c4, 12 %c5\n  \
         block c0:\n    store i8 0 @out\n    ret void\n  \
         block c1:\n    store i8 1 @out\n    ret void\n  \
         block c2:\n    store i8 2 @out\n    ret void\n  \
         block c3:\n    store i8 3 @out\n    ret void\n  \
         block c4:\n    store i8 4 @out\n    ret void\n  \
         block c5:\n    store i8 5 @out\n    ret void\n  \
         block def:\n    store i8 99 @out\n    ret void\n",
    );
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[("sel", 0x20), ("out", 0x28), ("main::1", 0x30)]),
        None,
    );
    assert!(!asm.contains("ADDWF 0xFF9"), "no PCL dispatch:\n{asm}");
    assert!(asm.contains("SUBLW 0x00"), "chain equality tests:\n{asm}");
}

#[test]
fn nonzero_base_switch_tables_with_padding() {
    // epic-cc#479: base 4, eight cases: the entry stride still indexes
    // the raw value, so values 0..3 need padding entries (targeting the
    // default) and the span covers the whole padded table.
    let cases = [
        "4 %c4", "5 %c5", "6 %c6", "7 %c7", "8 %c8", "9 %c9", "10 %c10", "11 %c11",
    ];
    let mut blocks = String::new();
    for i in 4..12 {
        blocks.push_str(&format!(
            "  block c{i}:\n    store i8 {i} @out\n    ret void\n"
        ));
    }
    let m = parse(&format!(
        "global sel i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i16 @sel\n    \
         switch i16 %1, default %def, cases {}\n{blocks}  \
         block def:\n    store i8 99 @out\n    ret void\n",
        cases.join(", ")
    ));
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[("sel", 0x20), ("out", 0x28), ("main::1", 0x30)]),
        None,
    );
    assert!(asm.contains("ADDWF 0xFF9,F,A"), "PCL dispatch:\n{asm}");
    assert!(
        asm.contains(".pcltbl "),
        "page-check marker with padded span:\n{asm}"
    );
    // 4 padding GOTOs plus one per case: base + n entries.
    assert_eq!(
        asm.matches("\n    GOTO ").count(),
        12,
        "padding plus case entries:\n{asm}"
    );
    assert!(asm.contains("SUBWF"), "low bound against the base:\n{asm}");
}

#[test]
fn i16_dense_switch_checks_the_high_byte() {
    // epic-cc#479: an i16 switch tables like the i8 shape, with a high-
    // byte reject up front (a nonzero high byte can never match a
    // 0..n-1 case). Pinned here rather than in the simulator: clang
    // folds an i16 switch of e2e-fixture shape into a const lookup
    // before isel runs.
    let cases = ["0 %c0", "1 %c1", "2 %c2", "3 %c3", "4 %c4", "5 %c5"];
    let mut blocks = String::new();
    for (i, _) in cases.iter().enumerate() {
        blocks.push_str(&format!(
            "  block c{i}:\n    store i8 {i} @out\n    ret void\n"
        ));
    }
    let m = parse(&format!(
        "global sel i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i16 @sel\n    \
         switch i16 %1, default %def, cases {}\n{blocks}  \
         block def:\n    store i8 99 @out\n    ret void\n",
        cases.join(", ")
    ));
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[("sel", 0x20), ("out", 0x28), ("main::1", 0x30)]),
        None,
    );
    assert!(asm.contains("ADDWF 0xFF9,F,A"), "PCL dispatch:\n{asm}");
    assert!(asm.contains(".pcltbl "), "page-check marker:\n{asm}");
    assert_eq!(
        asm.matches("\n    GOTO ").count(),
        6,
        "one absolute GOTO per case:\n{asm}"
    );
    assert!(
        asm.matches("\n    BNZ ").count() >= 1,
        "high-byte reject for i16 values above 255:\n{asm}"
    );
}

#[test]
fn negative_base_switch_keeps_the_compare_chain() {
    // A dense negative-base switch must never table: the entry math
    // indexes the raw value forward from zero, so a negative base
    // would jump outside the table while every check passes. The
    // chain compares full two's-complement values and handles it.
    let cases = ["-8 %c0", "-7 %c1", "-6 %c2", "-5 %c3", "-4 %c4", "-3 %c5"];
    let mut blocks = String::new();
    for (i, _) in cases.iter().enumerate() {
        blocks.push_str(&format!(
            "  block c{i}:\n    store i8 {i} @out\n    ret void\n"
        ));
    }
    let m = parse(&format!(
        "global sel i16\nglobal out i8\n\
         fn main(void) ()\n  block entry:\n    %1 = load i16 @sel\n    \
         switch i16 %1, default %def, cases {}\n{blocks}  \
         block def:\n    store i8 99 @out\n    ret void\n",
        cases.join(", ")
    ));
    let asm = select(
        &PIC18F4550,
        &m,
        &addrs(&[("sel", 0x20), ("out", 0x28), ("main::1", 0x30)]),
        None,
    );
    assert!(!asm.contains("ADDWF 0xFF9"), "no PCL dispatch:\n{asm}");
    assert!(!asm.contains(".pcltbl"), "no table marker:\n{asm}");
}

#[test]
fn icmp_result_needs_no_literal_diamond() {
    // epic-cc#503's sink: the comparison result reaches its byte slot
    // through `BRA join / MOVLW 0 / BRA out / MOVLW 1`. Clearing the slot
    // before the compare leaves the branches selecting between "already
    // zero" and one `INCF`, so no literal is loaded at all.
    let m = parse(
        "global a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    \
         %1 = load i8 @a\n    %2 = icmp eq i8 %1, 4\n    store i8 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("out", 0x21),
        ("main::1", 0x22),
        ("main::2", 0x23),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("MOVLW 0x00") && !asm.contains("MOVLW 0x01"),
        "no literal arms:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (av, expect) in [(4u8, 1u8), (3, 0), (5, 0)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x20] = av;
        p.run(200);
        assert_eq!(p.ram()[0x21], expect, "eq({av},4)");
    }
}

#[test]
fn icmp_eq_zero_skips_the_literal_subtract() {
    // `x == 0` consumes only Z, and `MOVF f,W` sets Z from `f` itself. The
    // `MOVLW 0x00`/`SUBWF` pair it replaces is the epic-cc#503 sink.
    let m = parse(
        "global x i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    \
         %1 = load i8 @x\n    %2 = icmp eq i8 %1, 0\n    store i8 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("x", 0x10),
        ("out", 0x11),
        ("main::1", 0x12),
        ("main::2", 0x13),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("MOVF 0x012,W,A"), "Z from the byte:\n{asm}");
    assert!(
        !asm.contains("SUBWF"),
        "a zero compare needs no subtract:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (xv, expect) in [(0u8, 1u8), (7, 0)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = xv;
        p.run(200);
        assert_eq!(p.ram()[0x11], expect, "eq({xv},0)");
    }
}

#[test]
fn icmp_ordering_predicates_also_skip_the_diamond() {
    // The pre-clear is sound for every predicate, not just eq/ne: it only
    // relies on `dst` overlapping nothing the compare reads.
    for (pred, av, expect) in [
        ("ult", 3u8, 1u8),
        ("ult", 4, 0),
        ("ugt", 5, 1),
        ("ugt", 4, 0),
        ("ule", 4, 1),
        ("ule", 5, 0),
        ("uge", 5, 1),
        ("uge", 3, 0),
        ("slt", 0xFF, 1),
        ("sge", 0xFF, 0),
        ("sgt", 0x80, 0),
        ("sle", 0x80, 1),
    ] {
        let m = parse(&format!(
            "global a i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    \
             %1 = load i8 @a\n    %2 = icmp {pred} i8 %1, 4\n    store i8 %2 @out\n    ret void\n"
        ));
        let addrs = addrs(&[
            ("a", 0x20),
            ("out", 0x21),
            ("main::1", 0x22),
            ("main::2", 0x23),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert!(
            !asm.contains("MOVLW 0x00") && !asm.contains("MOVLW 0x01"),
            "{pred} has no literal diamond:\n{asm}"
        );
        let words = asm::assemble_pic18(&asm);
        for (v, e) in [
            (av, expect),
            (4, u8::from(matches!(pred, "ule" | "uge" | "sle" | "sge"))),
        ] {
            let mut p = pic14_sim::Pic18::new(words.clone());
            step_past_start(&mut p, start_steps(&asm));
            p.ram_mut()[0x20] = v;
            p.run(200);
            assert_eq!(p.ram()[0x21], e, "{pred}({v}, 4)");
        }
    }
}

#[test]
fn icmp_i32_result_needs_no_literal_diamond() {
    // The widest equality path: four byte compares feeding one result slot.
    // Both operands are registers so the literal arms the ticket removes
    // are the only `MOVLW`s a correct lowering could emit.
    let m = parse(
        "global a i32\nglobal b i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    \
         %1 = load i32 @a\n    %2 = load i32 @b\n    %3 = icmp eq i32 %1, %2\n    \
         store i8 %3 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x24),
        ("out", 0x28),
        ("main::1", 0x30),
        ("main::2", 0x34),
        ("main::3", 0x38),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("MOVLW 0x00") && !asm.contains("MOVLW 0x01"),
        "no literal arms:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (bytes, expect) in [([2u8, 1, 0, 0], 1u8), ([2, 1, 0, 1], 1), ([0, 0, 0, 0], 1)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        for (i, v) in bytes.iter().enumerate() {
            p.ram_mut()[0x20 + i] = *v;
            p.ram_mut()[0x24 + i] = *v;
        }
        p.run(300);
        assert_eq!(p.ram()[0x28], expect, "eq equal inputs {bytes:?}");
    }
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x20] = 1;
    p.ram_mut()[0x24] = 2;
    p.run(300);
    assert_eq!(p.ram()[0x28], 0, "eq differing inputs");
}

#[test]
fn icmp_i32_ordering_result_needs_no_literal_diamond() {
    // The ordering shape chains four `emit_cmp_branch` calls through
    // tie-break labels, so the pre-clear has to survive every one of them.
    let m = parse(
        "global a i32\nglobal b i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    \
         %1 = load i32 @a\n    %2 = load i32 @b\n    %3 = icmp ugt i32 %1, %2\n    \
         store i8 %3 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x24),
        ("out", 0x28),
        ("main::1", 0x30),
        ("main::2", 0x34),
        ("main::3", 0x38),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("MOVLW 0x00") && !asm.contains("MOVLW 0x01"),
        "no literal arms:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (av, bv, expect) in [
        ([0u8, 0, 1, 0], [0u8, 0, 0, 0], 1u8),
        ([0, 0, 0, 0], [0, 0, 1, 0], 0),
        ([5, 0, 0, 0], [5, 0, 0, 0], 0),
        ([0, 0, 2, 0], [0, 0, 1, 0], 1),
    ] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        for (i, v) in av.iter().enumerate() {
            p.ram_mut()[0x20 + i] = *v;
        }
        for (i, v) in bv.iter().enumerate() {
            p.ram_mut()[0x24 + i] = *v;
        }
        p.run(300);
        assert_eq!(p.ram()[0x28], expect, "ugt({av:?}, {bv:?})");
    }
}

#[test]
fn icmp_result_aliasing_an_operand_keeps_the_diamond() {
    // The guard's refusal check. The aliasing is written into the address
    // map rather than reached through `alloc`, so this pins the fallback
    // itself: clearing a slot the compare reads would destroy the operand
    // before it is compared.
    let m = parse(
        "global a i8\nfn main(void) ()\n  block entry:\n    \
         %1 = load i8 @a\n    %2 = icmp eq i8 %1, 4\n    store i8 %2 @a\n    ret void\n",
    );
    let addrs = addrs(&[("a", 0x20), ("main::1", 0x21), ("main::2", 0x21)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("MOVLW 0x00"), "join form kept:\n{asm}");
    assert!(asm.contains("MOVLW 0x01"), "both arms kept:\n{asm}");
    assert!(!asm.contains("CLRF 0x021"), "no clear:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    for (av, expect) in [(4u8, 1u8), (3, 0), (9, 0)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x20] = av;
        p.run(200);
        assert_eq!(p.ram()[0x20], expect, "in-place eq({av},4)");
    }
}

#[test]
fn icmp_ne_zero_skips_the_literal_subtract() {
    let m = parse(
        "global x i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    \
         %1 = load i8 @x\n    %2 = icmp ne i8 %1, 0\n    store i8 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("x", 0x10),
        ("out", 0x11),
        ("main::1", 0x12),
        ("main::2", 0x13),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("MOVF 0x012,W,A"), "Z from the byte:\n{asm}");
    assert!(
        !asm.contains("SUBWF"),
        "a zero compare needs no subtract:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (xv, expect) in [(0u8, 0u8), (7, 1)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = xv;
        p.run(200);
        assert_eq!(p.ram()[0x11], expect, "ne({xv},0)");
    }
}

#[test]
fn icmp_eq_zero_uses_the_banked_operand_form() {
    // Place both operands above the access bank: the emitted form must
    // carry the banked `,B` operand and its `MOVLB`, the shape every
    // menu-demo site actually takes, not only the `,A` one.
    let m = parse(
        "global x i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    \
         %1 = load i8 @x\n    %2 = icmp eq i8 %1, 0\n    store i8 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("x", 0x112),
        ("out", 0x113),
        ("main::1", 0x114),
        ("main::2", 0x115),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    // The operand prints as the bank-relative byte plus the `,B` bit.
    assert!(
        asm.contains("MOVF 0x014,W,B"),
        "banked zero compare:\n{asm}"
    );
    assert!(
        asm.contains("MOVLB 0x1"),
        "bank selected for the load:\n{asm}"
    );
    assert!(!asm.contains("SUBWF"), "no subtract:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    for (xv, expect) in [(0u8, 1u8), (7, 0)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x112] = xv;
        p.run(200);
        assert_eq!(p.ram()[0x113], expect, "banked eq({xv},0)");
    }
}

#[test]
fn icmp_i32_eq_zero_converts_every_lane() {
    let m = parse(
        "global x i32\nglobal out i8\nfn main(void) ()\n  block entry:\n    \
         %1 = load i32 @x\n    %2 = icmp eq i32 %1, 0\n    store i8 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("x", 0x20),
        ("out", 0x24),
        ("main::1", 0x28),
        ("main::2", 0x2C),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(!asm.contains("SUBWF"), "no lane subtracts:\n{asm}");
    // `MOVF` for each of the four compared lanes; the two `MOVLW` that
    // remain belong to the shared 0/1 materialization, not the compare.
    assert_eq!(asm.matches("MOVF 0x0").count(), 4, "one per byte:\n{asm}");
    assert_eq!(asm.matches("BNZ").count(), 4, "one branch per byte:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    for (bytes, expect) in [
        ([0u8, 0, 0, 0], 1u8),
        ([0, 0, 0, 1], 0),
        ([1, 0, 0, 0], 0),
        ([0, 0, 1, 0], 0),
    ] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        for (i, b) in bytes.iter().enumerate() {
            p.ram_mut()[0x20 + i] = *b;
        }
        p.run(300);
        assert_eq!(p.ram()[0x24], expect, "eq({bytes:?}, 0)");
    }
}

#[test]
fn icmp_ordering_against_zero_keeps_the_carry_producing_subtract() {
    // The gate is "the consumer reads only Z", not "the literal is zero":
    // `uge`/`ult`/`ugt`/`ule` branch on C and `slt`/`sge`/`sgt`/`sle` on
    // N/OV, none of which `MOVF` writes. Dropping the subtract there would
    // leave those branches reading a stale flag.
    for (pred, xv, expect) in [
        ("uge", 0u8, 1u8),
        ("uge", 7, 1),
        ("ult", 0, 0),
        ("ult", 7, 0),
        ("ugt", 0, 0),
        ("ugt", 7, 1),
        ("ule", 0, 1),
        ("ule", 7, 0),
        ("sge", 0xFF, 0),
        ("slt", 0xFF, 1),
        ("sgt", 0xFF, 0),
        ("sle", 0xFF, 1),
    ] {
        let m = parse(&format!(
            "global x i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    \
             %1 = load i8 @x\n    %2 = icmp {pred} i8 %1, 0\n    store i8 %2 @out\n    ret void\n"
        ));
        let addrs = addrs(&[
            ("x", 0x10),
            ("out", 0x11),
            ("main::1", 0x12),
            ("main::2", 0x13),
        ]);
        let asm = select(&PIC18F4550, &m, &addrs, None);
        assert!(
            asm.contains("SUBWF 0x012,W,A"),
            "{pred} keeps its subtract:\n{asm}"
        );
        let words = asm::assemble_pic18(&asm);
        let mut p = pic14_sim::Pic18::new(words);
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = xv;
        p.run(200);
        assert_eq!(p.ram()[0x11], expect, "{pred}({xv}, 0)");
    }
}

#[test]
fn icmp_i16_eq_zero_compares_the_high_byte_with_movf_but_keeps_its_subtract() {
    // A multi-byte equality still needs the subtract on bytes whose value
    // can make the difference, but a byte the literal leaves zero can use
    // `MOVF`. The high byte is the zero one here, so it converts and the
    // low byte (literal 0x07) does not.
    let m = parse(
        "global x i16\nglobal out i8\nfn main(void) ()\n  block entry:\n    \
         %1 = load i16 @x\n    %2 = icmp eq i16 %1, 7\n    store i8 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("x", 0x10),
        ("out", 0x12),
        ("main::1", 0x13),
        ("main::2", 0x14),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("MOVF 0x014,W,A"),
        "zero high byte compares via MOVF:\n{asm}"
    );
    assert!(
        asm.contains("MOVLW 0x07"),
        "non-zero low byte still stages its literal:\n{asm}"
    );
    assert!(
        asm.contains("SUBWF 0x013,W,A"),
        "non-zero low byte still subtracts:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (lo, hi, expect) in [(7u8, 0u8, 1u8), (6, 0, 0), (7, 1, 0), (0, 0, 0)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = lo;
        p.ram_mut()[0x11] = hi;
        p.run(300);
        assert_eq!(p.ram()[0x12], expect, "eq(0x{hi:02X}{lo:02X}, 7)");
    }
}

#[test]
fn icmp_nonzero_byte_compare_still_subtracts_the_literal() {
    let m = parse(
        "global x i8\nglobal out i8\nfn main(void) ()\n  block entry:\n    \
         %1 = load i8 @x\n    %2 = icmp eq i8 %1, 4\n    store i8 %2 @out\n    ret void\n",
    );
    let addrs = addrs(&[
        ("x", 0x10),
        ("out", 0x11),
        ("main::1", 0x12),
        ("main::2", 0x13),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("MOVLW 0x04"), "literal staged:\n{asm}");
    assert!(asm.contains("SUBWF 0x012,W,A"), "subtract kept:\n{asm}");
}

#[test]
fn icmp_preclears_the_result_for_a_resolved_pointer_operand() {
    // epic-cc#519. The operand is a pointer VALUE (`inttoptr` seeds a
    // runtime-address slot, so `%a` has a `resolved` entry), which the guard
    // used to refuse wholesale, leaving the literal diamond. Fails against
    // the pre-patch source, so it pins the change.
    let m = parse(
        "global off i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %o = load i16 @off\n\
             %a = inttoptr i16 %o to ptr\n\
             %c = icmp eq ptr %a, 0\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("off", 0x120),
        ("main::o", 0x131),
        ("main::a", 0x133),
        ("main::c", 0x140),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        asm.contains("CLRF 0x040,B") || asm.contains("CLRF 0x040,A"),
        "a resolved pointer operand must let the result pre-clear:\n{asm}"
    );
    // The predicate must still hold: 1 iff the address is zero.
    let words = asm::assemble_pic18(&asm);
    for (val, expect) in [(0u16, 1u8), (0x0123, 0)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x120] = (val & 0xFF) as u8;
        p.ram_mut()[0x121] = (val >> 8) as u8;
        p.run(500);
        assert_eq!(p.ram()[0x140], expect, "icmp eq(ptr {val:#06x}, 0)");
    }
}

#[test]
fn join_agreement_elides_the_redundant_movlb() {
    // Both arms leave BSR at bank 0, so the merge block's banked store
    // must not re-select it. Without join tracking this emits 4 MOVLBs
    // (entry, t, f, merge); with it, exactly 1. Both paths run in the
    // simulator to prove the elision is sound, not just present.
    let m = parse(
        "global c i8\nglobal g i8\nglobal h i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @c\n    store i8 7 @g\n    br i1 %1 t f\n  block t:\n    store i8 8 @g\n    br merge\n  block f:\n    store i8 9 @g\n    br merge\n  block merge:\n    store i8 10 @h\n    ret void\n",
    );
    let addrs = addrs(&[("c", 0x10), ("g", 0x090), ("h", 0x091), ("main::1", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        1,
        "one MOVLB for the whole diamond:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (c, expect_g) in [(1u8, 8u8), (0, 9)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = c;
        p.run(500);
        assert!(p.halted());
        assert_eq!(p.ram()[0x090], expect_g, "cond={c} took the wrong arm");
        assert_eq!(p.ram()[0x091], 10, "merge store lost on cond={c}");
    }
}

#[test]
fn icmp_declines_when_the_result_slot_is_what_operand_b_reads() {
    // The soundness direction: `read` must contain every byte the compare
    // reads, so a result slot landing on them must decline. B's read here is
    // the resolved pointer's own slot bytes.
    let m = parse(
        "global off i16\n\
         fn main(void) ()\n\
           block entry:\n\
             %o = load i16 @off\n\
             %a = inttoptr i16 %o to ptr\n\
             %c = icmp eq ptr %a, 0\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("off", 0x120),
        ("main::o", 0x131),
        ("main::a", 0x133),
        ("main::c", 0x133), // the slot B's read comes from
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        !asm.contains("CLRF 0x033") && !asm.contains("CLRF 0x133"),
        "the guard must not clear a slot operand B is read from:\n{asm}"
    );
}

#[test]
fn join_disagreement_keeps_the_movlb() {
    // Arms leave BSR at different banks, so the merge block must
    // re-select. Total is 3: entry selects 0, f selects 1, merge
    // re-selects 0. t's store elides through entry agreement.
    let m = parse(
        "global c i8\nglobal g i8\nglobal k i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @c\n    store i8 7 @g\n    br i1 %1 t f\n  block t:\n    store i8 8 @g\n    br merge\n  block f:\n    store i8 9 @k\n    br merge\n  block merge:\n    store i8 10 @g\n    ret void\n",
    );
    let addrs = addrs(&[("c", 0x10), ("g", 0x090), ("k", 0x190), ("main::1", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        3,
        "entry, f-arm, and merge each select:\n{asm}"
    );
    assert!(
        block_section(&asm, "main_Lmerge").contains("MOVLB 0x0"),
        "merge must re-select bank 0:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (c, expect_k) in [(1u8, 0u8), (0, 9)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x10] = c;
        p.run(500);
        assert!(p.halted());
        assert_eq!(p.ram()[0x090], 10, "merge store lost on cond={c}");
        assert_eq!(p.ram()[0x190], expect_k, "k wrong on cond={c}");
    }
}

#[test]
fn loop_header_join_stays_unknown() {
    // The latch back-edge is unrecorded when the header emits, so the
    // header must not inherit the entry block's bank: its store
    // re-selects. c=0 exits immediately; c=1 would loop forever, so
    // only the exiting path runs.
    let m = parse(
        "global c i8\nglobal g i8\nglobal k i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @c\n    store i8 7 @g\n    br loop\n  block loop:\n    store i8 8 @k\n    br i1 %1 loop exit\n  block exit:\n    store i8 9 @g\n    ret void\n",
    );
    let addrs = addrs(&[("c", 0x10), ("g", 0x090), ("k", 0x190), ("main::1", 0x12)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        block_section(&asm, "main_Lloop").contains("MOVLB 0x1"),
        "header must select bank 1 despite entry leaving 0:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.ram_mut()[0x10] = 0;
    p.run(500);
    assert!(p.halted());
    assert_eq!(p.ram()[0x190], 8, "loop body store lost");
    assert_eq!(p.ram()[0x090], 9, "exit store lost");
}

#[test]
fn dirty_terminator_poison_single_pred_taken_target() {
    // The reviewer's trigger, end to end: the t edge exits the entry
    // block before the f edge's phi copies run, and those copies select
    // bank 1 while the taken path never selected anything (the cond
    // load stages through MOVFF). Recording one end state per block
    // would agree the taken target to bank 1 and elide its select,
    // storing through the wrong bank. The dirty flag forces unknown
    // instead: 2 MOVLBs (phi copy, taken store). Without it the count
    // is 1 and the simulator catches the misbanked store.
    let m = parse(
        "global cond i8\nglobal out1 i8\nfn main(void) ()\n  block entry:\n    %1 = load i8 @cond\n    br i1 %1 t f\n  block t:\n    store i8 1 @out1\n    ret void\n  block f:\n    %2 = phi i8 9 entry\n    store i8 %2 @out1\n    ret void\n",
    );
    let addrs = addrs(&[
        ("cond", 0x090),
        ("out1", 0x190),
        ("main::1", 0x12),
        ("main::2", 0x191),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        2,
        "phi copy and taken store each select:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (c, expect) in [(1u8, 1u8), (0, 9)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x090] = c;
        p.run(500);
        assert!(p.halted());
        assert_eq!(p.ram()[0x190], expect, "out1 wrong on cond={c}");
    }
}

#[test]
fn low_priority_epilogue_restores_bsr_after_the_banked_w_restore() {
    // The W save slot lives in banked RAM, so its restore selects the
    // save bank; without the re-restore that follows, main would resume
    // against the save bank instead of its own (epic-cc#534 makes
    // tracked agreement load-bearing there).
    let m = parse(
        "fn hi(void) [isr] [irq1] ()\n  block entry:\n    ret void\n\
         fn lo(void) [isr] [irq2] ()\n  block entry:\n    ret void\n\
         fn main(void) ()\n  block entry:\n    ret void\n",
    );
    let asm =
        isel_pic18::select_with_locs(&PIC18F4550, &m, &addrs(&[]), Some(0x140), None, Some(0x150))
            .0;
    let w = asm.find("MOVF 0x140, W, B").expect("banked W restore");
    let after_w = &asm[w..];
    let re = after_w
        .find("MOVFF 0x142, 0xFE0")
        .expect("BSR re-restore after the W restore");
    assert!(
        after_w[re..].contains("RETFIE"),
        "RETFIE follows the re-restore:\n{asm}"
    );
}

#[test]
fn ram_global_over_255_bytes_reads_across_the_bank_boundary() {
    // epic-cc#608: a 259-byte RAM global must place and address through
    // the 0x200 hardware-bank boundary. Constant field offsets fold to
    // absolute addresses; the sim asserts one byte per region: below,
    // near, and past offset 255.
    let mut m = parse(
        "global g i8\nglobal o0 i8\nglobal o1 i8\nglobal o2 i8\nglobal o3 i8\nfn main(void) ()\n  block entry:\n\
           %p0 = gep @g +0\n    %v0 = load i8 %p0\n    store i8 %v0 @o0\n\
           %p1 = gep @g +199\n    %v1 = load i8 %p1\n    store i8 %v1 @o1\n\
           %p2 = gep @g +256\n    %v2 = load i8 %p2\n    store i8 %v2 @o2\n\
           %p3 = gep @g +258\n    %v3 = load i8 %p3\n    store i8 %v3 @o3\n\
           ret void\n",
    );
    for g in &mut m.globals {
        if g.name == "g" {
            g.size = 259;
        }
    }
    let addrs = addrs(&[
        ("g", 0x100),
        ("o0", 0x250),
        ("o1", 0x251),
        ("o2", 0x252),
        ("o3", 0x253),
        ("main::v0", 0x254),
        ("main::v1", 0x255),
        ("main::v2", 0x256),
        ("main::v3", 0x257),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x100] = 0xA5;
    p.ram_mut()[0x100 + 199] = 0x5A;
    p.ram_mut()[0x100 + 256] = 0x77;
    p.ram_mut()[0x100 + 258] = 0x99;
    p.run(2000);
    assert!(p.halted(), "program must run to completion");
    assert_eq!(p.ram()[0x250], 0xA5, "offset 0");
    assert_eq!(p.ram()[0x251], 0x5A, "offset 199");
    assert_eq!(p.ram()[0x252], 0x77, "offset 256, past the old ceiling");
    assert_eq!(p.ram()[0x253], 0x99, "offset 258, across the bank boundary");
}

#[test]
fn bodies_concatenate_in_module_order_even_when_emission_reorders() {
    // main calls a helper defined after it. The carry analysis (epic-cc#495)
    // must emit helper first, but the output stream keeps master's order:
    // helper's body may not float above main's.
    let m = parse(
        "global a i8\nglobal b i8\nglobal c i8\nglobal d i8\nglobal e i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @a\n\
             %2 = load i8 @b\n\
             %3 = add i8 %1, %2\n\
             call void @helper()\n\
             store i8 %3 @c\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             %1 = load i8 @d\n\
             store i8 %1 @e\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x21),
        ("c", 0x22),
        ("d", 0x23),
        ("e", 0x24),
        ("main::1", 0x30),
        ("main::2", 0x31),
        ("main::3", 0x32),
        ("helper::1", 0x33),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let main_at = asm.find("\nmain:").expect("main label");
    let helper_at = asm.find("\nhelper:").expect("helper label");
    assert!(
        helper_at > main_at,
        "helper body must stay after main:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x20] = 3;
    p.ram_mut()[0x21] = 4;
    p.run(500);
    assert!(p.halted());
    assert_eq!(p.ram()[0x22], 7, "the post-call store must land");
}

#[test]
fn a_provable_callee_exit_bank_carries_across_the_call() {
    // f ends on bank 2 on its only return path, so main's tracked bank
    // after `CALL f` is 2 and the bank-2 store re-selects nothing.
    // Without carry this emits a MOVLB after the call; with it, the whole
    // module has exactly the one MOVLB f's own body needs.
    let m = parse(
        "global a i8\nglobal b i8\nglobal c i8\nglobal d i8\nglobal g i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @a\n\
             %2 = load i8 @b\n\
             %3 = add i8 %1, %2\n\
             call void @f()\n\
             store i8 7 @g\n\
             ret void\n\
         fn f(void) ()\n\
           block entry:\n\
             %1 = load i8 @c\n\
             %2 = load i8 @d\n\
             %3 = add i8 %1, %2\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x21),
        ("c", 0x22),
        ("d", 0x23),
        ("g", 0x290), // bank 2: the post-call store under test
        ("main::1", 0x30),
        ("main::2", 0x31),
        ("main::3", 0x32),
        ("f::1", 0x210), // bank 2
        ("f::2", 0x211), // bank 2
        ("f::3", 0x212), // bank 2: f's add dst selects the bank it exits on
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        1,
        "only f's own MOVLB; main's post-call store carries bank 2:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    step_past_start(&mut p, start_steps(&asm));
    p.ram_mut()[0x20] = 3;
    p.ram_mut()[0x21] = 4;
    p.ram_mut()[0x22] = 3; // c: f's add must also produce 7
    p.ram_mut()[0x23] = 4; // d
    p.run(500);
    assert!(p.halted());
    assert_eq!(p.ram()[0x212], 7, "f's add landed in f's slot");
    assert_eq!(
        p.ram()[0x290],
        7,
        "main's store landed in bank 2 without re-selecting"
    );
}

#[test]
fn a_terminator_selected_callee_exit_keeps_the_post_call_movlb() {
    // f's phi copies land in a BANKED merge slot, so both predecessor
    // terminators select a bank while lowering their edge's copy
    // (bsr_dirty), the exit join is unknown, and main must still
    // re-select after the call. The incoming values themselves live in
    // slots on two different banks (q1 bank 1, q2 bank 2). (A mid-block
    // bank select would NOT do this: it is cleared before the
    // terminator lowering and leaves the end state provable.)
    let m = parse(
        "global c i8\nglobal q1 i8\nglobal q2 i8\nglobal g i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f()\n\
             store i8 7 @g\n\
             ret void\n\
         fn f(void) ()\n\
           block entry:\n\
             %1 = load i8 @c\n\
             br i1 %1 t u\n\
           block t:\n\
             br merge\n\
           block u:\n\
             br merge\n\
           block merge:\n\
             %2 = phi i8 @q1 t @q2 u\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("c", 0x20),
        ("q1", 0x110), // bank 1: the t-edge incoming value's slot
        ("q2", 0x290), // bank 2: the u-edge incoming value's slot
        ("g", 0x090),  // bank 0: the post-call store must re-select it
        ("f::1", 0x30),
        ("f::2", 0x110), // bank 1: the banked phi merge slot
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        3,
        "each edge's copy selects the merge slot's bank 1 and main's \
         re-select; nothing carries:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for c in [1u8, 0] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x20] = c;
        p.run(500);
        assert!(p.halted());
        assert_eq!(p.ram()[0x090], 7, "main's store lost on cond={c}");
    }
}

#[test]
fn disagreeing_callee_returns_keep_the_post_call_movlb() {
    // f's two arms return on different banks, so the exit join is
    // unknown and main re-selects. Both arms run in the simulator.
    let m = parse(
        "global a i8\nglobal c i8\nglobal g i8\nglobal k i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @a\n\
             call void @f()\n\
             store i8 7 @g\n\
             ret void\n\
         fn f(void) ()\n\
           block entry:\n\
             %1 = load i8 @c\n\
             br i1 %1 t u\n\
           block t:\n\
             store i8 1 @k\n\
             ret void\n\
           block u:\n\
             store i8 2 @slot\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("c", 0x21),
        ("g", 0x090),    // bank 0: main must re-select it after the call
        ("k", 0x110),    // bank 1: arm t exits here
        ("slot", 0x290), // bank 2: arm u exits here
        ("main::1", 0x30),
        ("f::1", 0x31),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        block_section(&asm, "main").contains("MOVLB 0x0"),
        "main must re-select bank 0 after the unknown exit:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (c, want_k, want_slot) in [(1u8, 1u8, 0u8), (0u8, 0u8, 2u8)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        step_past_start(&mut p, start_steps(&asm));
        p.ram_mut()[0x21] = c; // c: f's branch condition picks the arm
        p.run(500);
        assert!(p.halted());
        assert_eq!(p.ram()[0x090], 7, "main's store lost on cond={c}");
        assert_eq!(p.ram()[0x110], want_k, "arm t's store wrong on cond={c}");
        assert_eq!(p.ram()[0x290], want_slot, "arm u's store wrong on cond={c}");
    }
}

#[test]
fn a_forward_defined_callee_with_provable_exit_still_carries() {
    // helper is defined after main. The map is filled by the buffered
    // reverse-topological emission, so the textual order must not matter.
    let m = parse(
        "global a i8\nglobal g i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @helper()\n\
             store i8 7 @g\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             store i8 1 @g\n\
             ret void\n",
    );
    let addrs = addrs(&[("a", 0x20), ("g", 0x090)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        1,
        "helper selects bank 0; main's store carries it:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.run(500);
    assert!(p.halted());
    assert_eq!(p.ram()[0x090], 7);
}

#[test]
fn a_recipe_callee_keeps_the_post_call_movlb() {
    // __mul_u16 has no Gen run: its body streams from the recipe in the
    // concat loop and its exit never enters the map, so main must
    // re-select after the call. The stub's alloca-only entry block must
    // not leak into the output as an empty label either.
    let m = parse(
        "global a i16\nglobal g i8\n\
         fn __mul_u16(i16) (a=i16, b=i16)\n\
           block entry:\n\
             %__scr = alloca 14\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             %2 = call i16 @__mul_u16(i16 %1, i16 %1)\n\
             store i8 7 @g\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("g", 0x090),
        ("main::1", 0x30),
        ("main::2", 0x32),
        ("__mul_u16::a", 0x40),
        ("__mul_u16::b", 0x42),
        ("__mul_u16::__scr", 0x50),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("    CALL __mul_u16"), "recipe call:\n{asm}");
    assert!(
        block_section(&asm, "main").contains("MOVLB 0x0"),
        "main must re-select after the recipe call:\n{asm}"
    );
    let lines: Vec<&str> = asm.lines().collect();
    let i = lines
        .iter()
        .position(|l| l.trim() == "__mul_u16:")
        .unwrap_or_else(|| panic!("recipe label missing:\n{asm}"));
    let next = lines.get(i + 1).copied().unwrap_or("");
    assert!(
        !next.trim().is_empty() && !next.trim_end().ends_with(':'),
        "stub entry block leaked as an empty label:\n{asm}"
    );
}
