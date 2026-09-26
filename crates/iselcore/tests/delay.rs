use iselcore::delay::{delay_cost, plan_delay, DELAY_MAX};

#[test]
fn every_small_count_is_exact() {
    for n in 0..=800u64 {
        let plan = plan_delay(n);
        assert_eq!(delay_cost(&plan), n, "plan for {n} must cost exactly {n}");
    }
}

#[test]
fn loop_boundaries_are_exact() {
    for n in [768u64, 769, 770, 771, 772, 773, 774, 775, 1543, 1544, 1545] {
        let plan = plan_delay(n);
        assert_eq!(delay_cost(&plan), n, "plan for {n} must cost exactly {n}");
    }
}

#[test]
fn two_level_span_is_exact() {
    for n in [
        2000u64, 10_000, 50_000, 100_000, 197_632, 197_633, 198_403, 198_404,
    ] {
        let plan = plan_delay(n);
        assert_eq!(delay_cost(&plan), n, "plan for {n} must cost exactly {n}");
    }
}

#[test]
fn three_level_span_is_exact() {
    for n in [
        198_405u64,
        198_406,
        396_041,
        396_042,
        1_000_000,
        12_000_000,
        50_000_000,
        DELAY_MAX - 1,
        DELAY_MAX,
    ] {
        let plan = plan_delay(n);
        assert_eq!(delay_cost(&plan), n, "plan for {n} must cost exactly {n}");
    }
}

#[test]
fn levels_stay_within_three_counters() {
    for n in [0u64, 1, 771, 772, 198_404, 198_405, DELAY_MAX] {
        let plan = plan_delay(n);
        let depth = plan.nests.iter().map(Vec::len).max().unwrap_or(0);
        assert!(depth <= 3, "plan for {n} needs {depth} counters");
        for nest in &plan.nests {
            for &c in nest {
                assert!((1..=256).contains(&c), "count {c} out of byte range");
            }
        }
        assert!(plan.tail_nops <= 3, "tail must be 0-3 NOPs");
    }
}

#[test]
#[should_panic(expected = "exceeds")]
fn past_the_ceiling_panics() {
    plan_delay(DELAY_MAX + 1);
}
