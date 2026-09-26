use device::PIC12F509;
use pic14_sim::PicBaseline;
use std::collections::HashMap;

fn cycles_for(n: u64) -> u64 {
    let m = ir::parse(&format!(
        "fn main(void) ()\n  block 0:\n    call void @_delay(i32 {n})\n    ret void\n"
    ));
    let addrs: HashMap<String, u16> = HashMap::new();
    let asm = isel_pic_baseline::select(&PIC12F509, &m, &addrs);
    isel_pic_baseline::verify_page_fit(&m, &asm, &addrs);
    let mut p = PicBaseline::with_device(&PIC12F509, asm::assemble_pic_baseline(&asm));
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
    let addrs: HashMap<String, u16> = HashMap::new();
    let _ = isel_pic_baseline::select(&PIC12F509, &m, &addrs);
}
