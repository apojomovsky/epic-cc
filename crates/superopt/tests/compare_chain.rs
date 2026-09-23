//! The unsigned borrow-chain compare: the flag contract `isel-pic18`'s fused
//! and materializing chain lowerings both rest on (epic-cc#621).
//!
//! Contract: after `n` lanes of `MOVF rhs{i},W` / (`SUBWF` then `SUBWFB`)
//! into the left operand's lane `i`, the final `C` is exactly `(lhs >= rhs)`
//! for the whole `8n`-bit value. The lane order is load-bearing and is the
//! trap the design first fell into: a borrow propagates *upward*, so the
//! chain must start at lane 0. The high-to-low form is built here too and
//! asserted to fail, so the reason the shipped direction is the shipped
//! direction is checked rather than assumed.
//!
//! The candidate alphabet is straight-line (no labels; see
//! `superopt::Candidate`), so the verifiable unit is the chain prefix: the
//! flag it leaves is the compare's answer. The branch structure around it is
//! covered instead by simulating the selector's own emitted asm in
//! `crates/isel-pic18/tests/isel_pic18.rs`.

use pic14_sim::Pic18;
use superopt::{verify, Candidate, Case, STATUS_ADDR, STATUS_C_BIT};

const A: usize = 0x020;
const B: usize = 0x028;

/// Cases over a curated operand sample, with the expected final carry the
/// chain must leave. `entry_c` is varied because the first `SUBWF` sets C
/// outright (so a stale carry must not matter) and `SUBWFB` consumes it.
fn cases(n: usize, lefts: &[u32], rights: &[u32], entry_c: &[bool]) -> Vec<Case> {
    let mut cases = Vec::new();
    let mask: u32 = if n == 4 {
        u32::MAX
    } else {
        (1u32 << (8 * n)) - 1
    };
    let mut allowed: Vec<usize> = (0..n).map(|i| A + i).collect();
    allowed.extend((0..n).map(|i| B + i));
    for &l in lefts {
        for &r in rights {
            let (l, r) = (l & mask, r & mask);
            for &c in entry_c {
                let mut pokes: Vec<(usize, u8)> =
                    (0..n).map(|i| (A + i, (l >> (8 * i)) as u8)).collect();
                pokes.extend((0..n).map(|i| (B + i, (r >> (8 * i)) as u8)));
                pokes.push((STATUS_ADDR, if c { STATUS_C_BIT } else { 0 }));
                cases.push(Case {
                    entry_w: 0,
                    pokes,
                    allowed_changes: allowed.clone(),
                    check: Box::new(move |sim: &Pic18| {
                        let carry = sim.ram()[STATUS_ADDR] & STATUS_C_BIT != 0;
                        carry == (l >= r)
                    }),
                });
            }
        }
    }
    cases
}

/// The lane instructions, spelled out because `Candidate` holds `&'static
/// str`: address `A` is 0x020 and `B` is 0x028, so lane `i` of the left
/// operand is `0x02{i}` and of the right one `0x02{8+i}`.
const LANES_2: [(&str, &str, &str); 2] = [
    ("movf 0x028,W,A", "subwf 0x020,W,A", "subwfb 0x020,W,A"),
    ("movf 0x029,W,A", "subwf 0x021,W,A", "subwfb 0x021,W,A"),
];
const LANES_4: [(&str, &str, &str); 4] = [
    ("movf 0x028,W,A", "subwf 0x020,W,A", "subwfb 0x020,W,A"),
    ("movf 0x029,W,A", "subwf 0x021,W,A", "subwfb 0x021,W,A"),
    ("movf 0x02A,W,A", "subwf 0x022,W,A", "subwfb 0x022,W,A"),
    ("movf 0x02B,W,A", "subwf 0x023,W,A", "subwfb 0x023,W,A"),
];

/// `lhs - rhs` lane by lane, low to high: `SUBWF` on lane 0, `SUBWFB` above.
fn chain_low_to_high(lanes: &[(&'static str, &'static str, &'static str)]) -> Candidate {
    let mut c: Candidate = Vec::new();
    for (i, (load, sub, subb)) in lanes.iter().enumerate() {
        c.push(*load);
        c.push(if i == 0 { *sub } else { *subb });
    }
    c
}

/// The same lanes in the order the design first proposed, high to low.
fn chain_high_to_low(lanes: &[(&'static str, &'static str, &'static str)]) -> Candidate {
    let mut c: Candidate = Vec::new();
    for (i, (load, sub, subb)) in lanes.iter().enumerate().rev() {
        c.push(*load);
        c.push(if i == lanes.len() - 1 { *sub } else { *subb });
    }
    c
}

/// The strict predicates' chain: the plain chain with an explicit borrow-in
/// seed of 1 at lane 0, whose final carry is `(a > b)`. This is the form
/// `emit_icmp_chain` emits for `ugt`/`ule`, so it is verified here rather
/// than only the plain `uge`/`ult` chain.
fn chain_with_borrow_in(lanes: &[(&'static str, &'static str, &'static str)]) -> Candidate {
    let mut c: Candidate = vec!["bcf 0xFD8,0,A"];
    for (load, _sub, subb) in lanes.iter() {
        c.push(*load);
        c.push(*subb);
    }
    c
}

#[test]
fn i16_strict_chain_leaves_the_strict_ordering_carry() {
    let lefts = [0x0000u32, 0x0001, 0x00FF, 0x0100, 0x0101, 0x1234, 0xFFFF];
    let rights = [0x0000u32, 0x00FF, 0x0100, 0x0102, 0x1234, 0xFFFF];
    let c = chain_with_borrow_in(&LANES_2);
    assert_eq!(c.len(), 5, "seed + 2 lanes");
    let mut cases = cases(2, &lefts, &rights, &[false, true]);
    // Same case set, strict expectation instead of `>=`.
    for case in &mut cases {
        let l = case.pokes[0].1 as u32 | ((case.pokes[1].1 as u32) << 8);
        let r = case.pokes[2].1 as u32 | ((case.pokes[3].1 as u32) << 8);
        case.check = Box::new(move |sim: &Pic18| {
            let carry = sim.ram()[STATUS_ADDR] & STATUS_C_BIT != 0;
            carry == (l > r)
        });
    }
    assert!(
        verify(&c, &cases),
        "the borrow-in chain must leave C = (lhs > rhs)"
    );
}

#[test]
fn i16_borrow_chain_leaves_the_ordering_carry() {
    let lefts = [
        0x0000u32, 0x0001, 0x00FF, 0x0100, 0x0101, 0x1234, 0x8000, 0xFFFF,
    ];
    let rights = [0x0000u32, 0x00FF, 0x0100, 0x0102, 0x1234, 0xFFFF];
    let c = chain_low_to_high(&LANES_2);
    assert_eq!(c.len(), 4, "2 lanes x (MOVF + SUBWF/SUBWFB)");
    assert!(
        verify(&c, &cases(2, &lefts, &rights, &[false, true])),
        "the low-to-high 16-bit chain must leave C = (lhs >= rhs)"
    );
}

#[test]
fn i32_borrow_chain_leaves_the_ordering_carry() {
    let lefts = [
        0x0000_0000u32,
        0x0000_FFFF,
        0x0100_0000,
        0x00FF_FFFF,
        0xFFFF_FFFF,
        0x1234_5678,
    ];
    let rights = [
        0x0000_0000u32,
        0x0000_FFFF,
        0x0100_0000,
        0x00FF_FFFF,
        0xFFFF_FFFF,
        0x1234_5679,
    ];
    let c = chain_low_to_high(&LANES_4);
    assert_eq!(c.len(), 8, "4 lanes x (MOVF + SUBWF/SUBWFB)");
    assert!(
        verify(&c, &cases(4, &lefts, &rights, &[false, true])),
        "the low-to-high 32-bit chain must leave C = (lhs >= rhs)"
    );
}

/// The design's first draft ran the lanes high to low. It is wrong, and
/// this pins why: the chain must not be "fixed" back to that order.
#[test]
fn the_high_to_low_chain_is_unsound() {
    let c = chain_high_to_low(&LANES_2);
    // 0x0100 vs 0x00FF decides at the *low* lane, which high-to-low reads
    // before the borrow from... the high lane it has not yet computed.
    assert!(
        !verify(
            &c,
            &cases(2, &[0x0100, 0x0102], &[0x00FF, 0x00FF], &[false])
        ),
        "the high-to-low form must fail on 0x0100 vs 0x00FF"
    );
}
