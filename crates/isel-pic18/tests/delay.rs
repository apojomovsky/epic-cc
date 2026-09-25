use device::PIC18F4550;
use pic14_sim::Pic18;
use std::collections::HashMap;

fn cycles_for(n: u64) -> u64 {
    let m = ir::parse(&format!(
        "fn main(void) ()\n  block 0:\n    call void @_delay(i32 {n})\n    ret void\n"
    ));
    let asm = isel_pic18::select(&PIC18F4550, &m, &HashMap::new(), None);
    let opts = outline::Options {
        stack_depth: PIC18F4550.stack_depth as usize,
        ir_depth: 1,
        ..outline::Options::default()
    };
    let asm = outline::factor(&asm, &opts);
    let mut p = Pic18::new(asm::assemble_pic18(&asm));
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
    let _ = isel_pic18::select(&PIC18F4550, &m, &HashMap::new(), None);
}

#[test]
fn repeated_delays_stay_exact_through_outlining() {
    // Six identical sites sit above outline's factoring threshold (four):
    // shared setup pairs or tails would add cycles per site. The split
    // labels keep every fragment at length one, so six sites take exactly
    // six times one site, tails included.
    let pair = ir::parse(
        "fn main(void) ()\n  block 0:\n    call void @_delay(i32 100)\n    call void @_delay(i32 100)\n    call void @_delay(i32 100)\n    call void @_delay(i32 100)\n    call void @_delay(i32 102)\n    call void @_delay(i32 102)\n    ret void\n",
    );
    let opts = outline::Options {
        stack_depth: PIC18F4550.stack_depth as usize,
        ir_depth: 1,
        ..outline::Options::default()
    };
    let asm = isel_pic18::select(&PIC18F4550, &pair, &HashMap::new(), None);
    let mut p = Pic18::new(asm::assemble_pic18(&outline::factor(&asm, &opts)));
    p.run(50_000_000);
    assert!(p.halted());
    let one = cycles_for(100);
    let base = cycles_for(0);
    let two = cycles_for(102);
    assert_eq!(p.cycles() - base, 4 * (one - base) + 2 * (two - base));
}

#[test]
fn delay_after_a_valued_call_keeps_the_result() {
    // A retval copy staged in `pending_copies` must drain before the loop
    // reuses the retval bytes, or the copy reads counter residue. The
    // delay(0) twin emits no loop, so the cycle delta is the loop itself.
    fn cycles_with_delay(n: u64) -> u64 {
        let m = ir::parse(&format!(
            "global out i8\n\
             fn id(i8) (0=i8)\n  block 0:\n    ret i8 %0\n\
             fn main(void) ()\n  block 0:\n    %1 = call i8 @id(i8 7)\n    call void @_delay(i32 {n})\n    store i8 %1 @out\n    ret void\n",
        ));
        let addrs: HashMap<String, u16> = [("out", 0x28), ("main::1", 0x30), ("id::0", 0x31)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        let asm = isel_pic18::select(&PIC18F4550, &m, &addrs, None);
        let opts = outline::Options {
            stack_depth: PIC18F4550.stack_depth as usize,
            ir_depth: 2,
            ..outline::Options::default()
        };
        let mut p = Pic18::new(asm::assemble_pic18(&outline::factor(&asm, &opts)));
        p.run(50_000_000);
        assert!(p.halted());
        assert_eq!(p.ram()[0x28], 7, "result must survive the delay");
        p.cycles()
    }
    assert_eq!(cycles_with_delay(50) - cycles_with_delay(0), 50);
}
