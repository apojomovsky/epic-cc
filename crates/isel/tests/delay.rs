use device::PIC16F877A;
use pic14_sim::Pic14;
use std::collections::HashMap;

fn cycles_for(n: u64) -> u64 {
    let m = ir::parse(&format!(
        "fn main(void) ()\n  block 0:\n    call void @_delay(i32 {n})\n    ret void\n"
    ));
    let asm = isel::select(&PIC16F877A, &m, &HashMap::new());
    let asm = schedule::schedule(&PIC16F877A, &asm);
    let asm = banking::assign_banks(&PIC16F877A, &asm);
    let asm = peephole::optimize(&asm);
    isel::verify_page_fit(&m, &asm);
    let mut p = Pic14::new(asm::assemble(&asm));
    p.run(50_000_000);
    assert!(p.halted(), "_delay({n}) program must halt");
    p.cycles()
}

#[test]
fn delay_takes_exactly_its_count_in_cycles() {
    let base = cycles_for(0);
    for n in [
        1u64, 2, 3, 4, 5, 7, 8, 10, 100, 255, 256, 500, 768, 769, 770, 771, 772, 773, 774, 1544,
        2000,
    ] {
        assert_eq!(
            cycles_for(n) - base,
            n,
            "_delay({n}) must take exactly {n} cycles"
        );
    }
}

#[test]
fn delay_large_counts_stay_exact() {
    let base = cycles_for(0);
    for n in [
        50_000u64, 198_403, 198_404, 198_405, 198_406, 400_000, 1_000_000,
    ] {
        assert_eq!(
            cycles_for(n) - base,
            n,
            "_delay({n}) must take exactly {n} cycles"
        );
    }
}

#[test]
#[should_panic(expected = "compile-time constant")]
fn delay_rejects_a_runtime_count() {
    let m =
        ir::parse("fn main(void) ()\n  block 0:\n    call void @_delay(i32 %r)\n    ret void\n");
    let _ = isel::select(&PIC16F877A, &m, &HashMap::new());
}

#[test]
#[should_panic(expected = "exactly one argument")]
fn delay_rejects_the_wrong_arity() {
    let m = ir::parse("fn main(void) ()\n  block 0:\n    call void @_delay()\n    ret void\n");
    let _ = isel::select(&PIC16F877A, &m, &HashMap::new());
}

#[test]
#[should_panic(expected = "at least 2 cycles")]
fn delay_below_the_pin_minimum_panics() {
    // The bank-0 pin itself costs 2 cycles, so requests below it are
    // unplannable on parts without common RAM: loud refusal instead of
    // a drifted count.
    let m = ir::parse("fn main(void) ()\n  block 0:\n    call void @_delay(i32 1)\n    ret void");
    let _ = isel::select(&device::PIC16F74, &m, &HashMap::new());
}

/// Cycle count plus final asm for `_delay(n)` on a part without common
/// RAM (epic-cc#800). `banked` stores a bank-1 global beside the delay,
/// the context delay-only programs never exercise: without it banking
/// emits zero selects and the test would falsely pass.
fn pinned_cycles_for(device: &'static device::Device, n: u64, banked: bool) -> (u64, String) {
    let body = if banked {
        format!(
            "global g i8\nfn main(void) ()\n  block 0:\n    store i8 7 @g\n    call void @_delay(i32 {n})\n    ret void\n"
        )
    } else {
        format!("fn main(void) ()\n  block 0:\n    call void @_delay(i32 {n})\n    ret void\n")
    };
    let m = ir::parse(&body);
    let mut addrs = HashMap::new();
    if banked {
        // 0xA1 is bank-1 GPR on both parts, so the store forces
        // nonzero-bank traffic through banking.
        addrs.insert("g".to_string(), 0xA1);
    }
    let asm = isel::select(device, &m, &addrs);
    let asm = schedule::schedule(device, &asm);
    let asm = banking::assign_banks(device, &asm);
    let asm = peephole::optimize(&asm);
    isel::verify_page_fit(&m, &asm);
    let mut p = Pic14::with_device(device, asm::assemble(&asm));
    p.run(50_000_000);
    assert!(p.halted(), "_delay({n}) program must halt");
    (p.cycles(), asm)
}

/// Every bank-1 assertion's guard: banking's own selects use the named
/// `STATUS, 5/6` form, while the #800 pin uses the `0x03` address form,
/// so this only matches selects banking inserted.
fn assert_banked_selects(asm: &str) {
    assert!(
        asm.lines().any(|l| {
            let t = l.trim();
            t.starts_with("BSF STATUS, 5")
                || t.starts_with("BCF STATUS, 5")
                || t.starts_with("BSF STATUS, 6")
                || t.starts_with("BCF STATUS, 6")
        }),
        "banking must see nonzero-bank traffic (got none):\n{asm}"
    );
}

#[test]
fn delay_stays_exact_without_common_ram() {
    // docs/39 buckets 2 (F74) and 1 (873A): the counters sit in banked
    // `isr_home_window` behind the 2-cycle bank-0 pin, so the smallest
    // plannable request is 2 and every delta is measured against it.
    for device in [&device::PIC16F74, &device::PIC16F873A] {
        let (base, _) = pinned_cycles_for(device, 2, false);
        for n in [3u64, 4, 5, 7, 8, 10, 100, 255, 256, 500, 2000] {
            let (cycles, _) = pinned_cycles_for(device, n, false);
            assert_eq!(
                cycles - base,
                n - 2,
                "_delay({n}) on {} must take exactly {n} cycles",
                device.name
            );
        }
    }
}

#[test]
fn delay_beside_bank1_stores_stays_exact() {
    // The #800 context: a bank-1 store ahead of the delay, exercising
    // the prologue selects delay-only programs never see. The exact
    // delta proves banking added nothing inside the pinned loop.
    for device in [&device::PIC16F74, &device::PIC16F873A] {
        let (base, base_asm) = pinned_cycles_for(device, 2, true);
        assert_banked_selects(&base_asm);
        for n in [100u64, 2000] {
            let (cycles, asm) = pinned_cycles_for(device, n, true);
            assert_banked_selects(&asm);
            assert_eq!(
                cycles - base,
                n - 2,
                "_delay({n}) beside a bank-1 store on {} must take exactly {n} cycles",
                device.name
            );
        }
    }
}
