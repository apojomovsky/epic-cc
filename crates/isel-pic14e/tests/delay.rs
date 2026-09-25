use asm::assemble_words;
use banking::assign_banks;
use device::PIC16F1937;
use peephole::optimize;
use pic14_sim::Pic14e;
use schedule::schedule;
use std::collections::HashMap;

fn cycles_for(n: u64) -> u64 {
    let m = ir::parse(&format!(
        "fn main(void) ()\n  block 0:\n    call void @_delay(i32 {n})\n    ret void\n"
    ));
    let asm = isel_pic14e::select(&PIC16F1937, &m, &HashMap::new());
    let asm = schedule(&PIC16F1937, &asm);
    let asm = assign_banks(&PIC16F1937, &asm);
    let asm = optimize(&asm);
    isel_pic14e::verify_page_fit(&m, &asm);
    let mut p = Pic14e::with_device(&PIC16F1937, assemble_words(&PIC16F1937, &asm));
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
    let _ = isel_pic14e::select(&PIC16F1937, &m, &HashMap::new());
}
