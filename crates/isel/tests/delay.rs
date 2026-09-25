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
#[should_panic(expected = "needs common RAM")]
fn delay_refuses_parts_without_common_ram() {
    // F74 class (docs/39 bucket 1) addresses the fixed region through a
    // bank, which would pad the loop with selects: loud refusal instead
    // of a drifted count.
    let m =
        ir::parse("fn main(void) ()\n  block 0:\n    call void @_delay(i32 100)\n    ret void\n");
    let _ = isel::select(&device::PIC16F74, &m, &HashMap::new());
}
