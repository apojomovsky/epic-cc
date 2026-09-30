//! Oracle specs for the MDB cross-check (epic-cc#712): each spec is one
//! superopt-landed sequence plus the exact case list its own test
//! verifies in-sim. The tests call these builders, and the `mdb_oracle`
//! bin feeds the same lists to hardware, so sim and MDB always judge the
//! same domain by construction, never by parallel reimplementation.

use crate::{Candidate, Case, STATUS_ADDR, STATUS_C_BIT};
use pic14_sim::Pic18;

const REG: usize = 0x020;

/// The landed shift-left-4 form (`SWAPF` + mask + store, 3 words against
/// the 8-word unrolled single-byte baseline).
pub fn shift_left_4_candidate() -> Candidate {
    vec!["swapf 0x020,W,A", "andlw 0xF0", "movwf 0x020,A"]
}

/// The full 120-case domain from the spec test: every low/high nibble
/// pair crossed with entry `W` and entry `C`.
pub fn shift_left_4_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for lo in [0x0u8, 0x1, 0x7, 0x8, 0xF] {
        for hi in [0x0u8, 0x3, 0x8, 0xF] {
            for w in [0x00u8, 0xFF, 0x2A] {
                for c in [false, true] {
                    let v = (hi << 4) | lo;
                    let expect = (lo << 4) & 0xFF;
                    let status = if c { STATUS_C_BIT } else { 0 };
                    cases.push(Case {
                        entry_w: w,
                        pokes: vec![(REG, v), (STATUS_ADDR, status)],
                        allowed_changes: vec![REG],
                        check: Box::new(move |sim: &Pic18| sim.ram()[REG] == expect),
                    });
                }
            }
        }
    }
    cases
}

/// PIC18 STATUS N bit (bit 4): set from bit 7 of ALU results, the flag
/// the signed-compare sign check (epic-cc#782) reads after `XORWF`.
const STATUS_N_BIT: u8 = 0x10;

/// The signed-chain sign check's novel flag belief (epic-cc#782):
/// `XORWF f,W` sets N exactly when the result's bit 7 is set. The
/// borrow lanes reuse long-landed `SUBWF`/`SUBWFB` semantics; only this
/// N bit is new, so only it gets a hardware spec.
pub fn xorwf_n_flag_candidate() -> Candidate {
    vec!["xorwf 0x020,W,A"]
}

/// Sign edges crossed with entry `W` and entry flags: bit 7 set/clear
/// on each side of the XOR, so a candidate that only works for one
/// polarity fails. Every case pokes STATUS because the batch poisons
/// RAM (unimplemented STATUS bits read 0 on silicon but keep poison
/// in-sim, so an unpoked STATUS can never byte-match); 0x00 and 0x1F
/// cover clear and set flag entry states with those bits clear.
pub fn xorwf_n_flag_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for a in [0x00u8, 0x01, 0x7F, 0x80, 0x81, 0xFE, 0xFF] {
        for w in [0x00u8, 0x01, 0x7F, 0x80, 0xFF] {
            for st in [0x00u8, 0x1F] {
                let r = w ^ a;
                let n = r & 0x80 != 0;
                cases.push(Case {
                    entry_w: w,
                    pokes: vec![(REG, a), (STATUS_ADDR, st)],
                    allowed_changes: vec![],
                    check: Box::new(move |sim: &Pic18| {
                        sim.w() == r && (sim.ram()[STATUS_ADDR] & STATUS_N_BIT != 0) == n
                    }),
                });
            }
        }
    }
    cases
}

/// Full `a` domain with a `W` sample: every bit-7 combination appears
/// many times over, deterministically.
pub fn xorwf_n_flag_nightly() -> Vec<Case> {
    let mut cases = Vec::new();
    for a in 0..=255u8 {
        for w in [0x00u8, 0xFF, 0x2A, 0x55] {
            for st in [0x00u8, 0x1F] {
                let r = w ^ a;
                let n = r & 0x80 != 0;
                cases.push(Case {
                    entry_w: w,
                    pokes: vec![(REG, a), (STATUS_ADDR, st)],
                    allowed_changes: vec![],
                    check: Box::new(move |sim: &Pic18| {
                        sim.w() == r && (sim.ram()[STATUS_ADDR] & STATUS_N_BIT != 0) == n
                    }),
                });
            }
        }
    }
    cases
}

/// One oracle spec: a landed candidate plus the case lists each tier
/// replays on hardware. `pr_cases` fits few sessions; `nightly_cases` is
/// the wider sweep. `chunk_cases` caps one HEX below 18F4550 flash and its
/// output table below 2 KB RAM; the batch builder asserts both, so a
/// future longer candidate fails loudly instead of silently truncating.
pub struct Spec {
    pub name: &'static str,
    pub candidate: fn() -> Candidate,
    pub pr_cases: fn() -> Vec<Case>,
    pub nightly_cases: fn() -> Vec<Case>,
    pub chunk_cases: usize,
}

/// The full acceptance list (epic-cc#712): every superopt-landed sequence.
pub fn all_specs() -> Vec<Spec> {
    vec![
        Spec {
            name: "shift-left-4",
            candidate: shift_left_4_candidate,
            pr_cases: shift_left_4_cases,
            nightly_cases: shift_left_4_cases,
            chunk_cases: 500,
        },
        Spec {
            name: "shift-right-4",
            candidate: shift_right_4_candidate,
            pr_cases: shift_right_4_hw_cases,
            nightly_cases: shift_right_4_hw_cases,
            chunk_cases: 500,
        },
        Spec {
            name: "single-shl-5",
            candidate: single_shl_5,
            pr_cases: single_shl_5_cases,
            nightly_cases: single_shl_5_cases,
            chunk_cases: 500,
        },
        Spec {
            name: "single-shl-6",
            candidate: single_shl_6,
            pr_cases: single_shl_6_cases,
            nightly_cases: single_shl_6_cases,
            chunk_cases: 500,
        },
        Spec {
            name: "single-shl-7",
            candidate: single_shl_7,
            pr_cases: single_shl_7_cases,
            nightly_cases: single_shl_7_cases,
            chunk_cases: 500,
        },
        Spec {
            name: "single-shr-5",
            candidate: single_shr_5,
            pr_cases: single_shr_5_cases,
            nightly_cases: single_shr_5_cases,
            chunk_cases: 500,
        },
        Spec {
            name: "single-shr-6",
            candidate: single_shr_6,
            pr_cases: single_shr_6_cases,
            nightly_cases: single_shr_6_cases,
            chunk_cases: 500,
        },
        Spec {
            name: "single-shr-7",
            candidate: single_shr_7,
            pr_cases: single_shr_7_cases,
            nightly_cases: single_shr_7_cases,
            chunk_cases: 500,
        },
        Spec {
            name: "shift-16bit-4",
            candidate: shift_16bit_4,
            pr_cases: shift_16bit_4_pr,
            nightly_cases: shift_16bit_4_nightly,
            chunk_cases: 380,
        },
        Spec {
            name: "shift-16bit-5",
            candidate: shift_16bit_5,
            pr_cases: shift_16bit_5_pr,
            nightly_cases: shift_16bit_5_nightly,
            chunk_cases: 380,
        },
        Spec {
            name: "shift-16bit-6",
            candidate: shift_16bit_6,
            pr_cases: shift_16bit_6_pr,
            nightly_cases: shift_16bit_6_nightly,
            chunk_cases: 380,
        },
        Spec {
            name: "shift-16bit-7",
            candidate: shift_16bit_7,
            pr_cases: shift_16bit_7_pr,
            nightly_cases: shift_16bit_7_nightly,
            chunk_cases: 380,
        },
        Spec {
            name: "shift-16bit-r4",
            candidate: shift_16bit_r4,
            pr_cases: shift_16bit_r4_pr,
            nightly_cases: shift_16bit_r4_nightly,
            chunk_cases: 380,
        },
        Spec {
            name: "shift-32bit-6",
            candidate: shift_32bit_6,
            pr_cases: shift_32bit_6_pr,
            nightly_cases: shift_32bit_6_nightly,
            chunk_cases: 250,
        },
        Spec {
            name: "shift-32bit-7",
            candidate: shift_32bit_7,
            pr_cases: shift_32bit_7_pr,
            nightly_cases: shift_32bit_7_nightly,
            chunk_cases: 250,
        },
        Spec {
            name: "bitmask-eq1",
            candidate: bitmask_eq1_candidate,
            pr_cases: bitmask_eq1_pr,
            nightly_cases: bitmask_eq1_nightly,
            chunk_cases: 500,
        },
        Spec {
            name: "bitmask-eq1-inv",
            candidate: bitmask_eq1_inv_candidate,
            pr_cases: bitmask_eq1_inv_pr,
            nightly_cases: bitmask_eq1_inv_nightly,
            chunk_cases: 500,
        },
        Spec {
            name: "bitmask-truthy",
            candidate: bitmask_truthy_candidate,
            pr_cases: bitmask_truthy_pr,
            nightly_cases: bitmask_truthy_nightly,
            chunk_cases: 500,
        },
        Spec {
            name: "bitmask-base",
            candidate: bitmask_base_candidate,
            pr_cases: bitmask_base_pr,
            nightly_cases: bitmask_base_nightly,
            chunk_cases: 500,
        },
        Spec {
            name: "xorwf-n-flag",
            candidate: xorwf_n_flag_candidate,
            pr_cases: xorwf_n_flag_cases,
            nightly_cases: xorwf_n_flag_nightly,
            chunk_cases: 500,
        },
    ]
}

const LANE: usize = 0x020;
const LO: usize = 0x020;
const HI: usize = 0x021;
pub const BASE32: usize = 0x020;
const W_SAMPLE: &[u8] = &[0x00, 0xFF, 0x2A];

/// Deterministic LCG shared by every hardware sample list: no RNG
/// dependency, so a failure replays exactly.
fn lcg_next(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    *state
}

/// The 4-bit single-lane right shift (moved from the spec test; the test
/// now calls this, so sim and hardware share the construction).
pub fn shift_right_4_candidate() -> Candidate {
    vec!["swapf 0x020,W,A", "andlw 0x0F", "movwf 0x020,A"]
}

/// Every byte value at a single `W`: the bit-pattern dimension.
pub fn shift_right_4_exhaustive() -> Vec<Case> {
    let all: Vec<u8> = (0..=255u8).collect();
    shift_right_4_cases_over(4, &all, &[0x00])
}

pub fn shift_right_4_cases_over(amount: u32, pairs: &[u8], w_values: &[u8]) -> Vec<Case> {
    let mut cases = Vec::new();
    for &lane in pairs {
        for &w in w_values {
            for c in [false, true] {
                let expect = lane.wrapping_shr(amount);
                let status = if c { STATUS_C_BIT } else { 0 };
                cases.push(Case {
                    entry_w: w,
                    pokes: vec![(LANE, lane), (STATUS_ADDR, status)],
                    allowed_changes: vec![LANE],
                    check: Box::new(move |sim: &Pic18| sim.ram()[LANE] == expect),
                });
            }
        }
    }
    cases
}

/// Full byte domain plus `W` independence on a 16-value subset: the
/// test's exhaustive list plus its `W` cross, sized to one chunk.
pub fn shift_right_4_hw_cases() -> Vec<Case> {
    let all: Vec<u8> = (0..=255u8).collect();
    let mut cases = shift_right_4_cases_over(4, &all, &[0x00]);
    let subset: Vec<u8> = [
        0x00, 0x01, 0x07, 0x08, 0x0F, 0x10, 0x1F, 0x3A, 0x55, 0x7E, 0x80, 0xA5, 0xC3, 0xE7, 0xF0,
        0xFF,
    ]
    .to_vec();
    cases.extend(shift_right_4_cases_over(4, &subset, W_SAMPLE));
    cases
}

pub fn single_cases_over(
    shift: fn(u8, u32) -> u8,
    amount: u32,
    pairs: &[u8],
    w_values: &[u8],
) -> Vec<Case> {
    let mut cases = Vec::new();
    for &lane in pairs {
        for &w in w_values {
            for c in [false, true] {
                let expect = shift(lane, amount);
                let status = if c { STATUS_C_BIT } else { 0 };
                cases.push(Case {
                    entry_w: w,
                    pokes: vec![(LANE, lane), (STATUS_ADDR, status)],
                    allowed_changes: vec![LANE],
                    check: Box::new(move |sim: &Pic18| sim.ram()[LANE] == expect),
                });
            }
        }
    }
    cases
}

pub fn single_all_bytes() -> Vec<u8> {
    (0..=u8::MAX).collect()
}

pub fn single_construction_shl(r: u32) -> Candidate {
    let mut c: Candidate = vec!["swapf 0x020,W,A", "andlw 0xF0", "movwf 0x020,A"];
    for _ in 4..r {
        c.push("bcf 0xFD8,0,A");
        c.push("rlcf 0x020,F,A");
    }
    c
}

pub fn single_construction_shr(r: u32) -> Candidate {
    let mut c: Candidate = vec!["swapf 0x020,W,A", "andlw 0x0F", "movwf 0x020,A"];
    for _ in 4..r {
        c.push("bcf 0xFD8,0,A");
        c.push("rrcf 0x020,F,A");
    }
    c
}

pub fn single_shl_5() -> Candidate {
    single_construction_shl(5)
}
pub fn single_shl_6() -> Candidate {
    single_construction_shl(6)
}
pub fn single_shl_7() -> Candidate {
    single_construction_shl(7)
}
pub fn single_shr_5() -> Candidate {
    single_construction_shr(5)
}
pub fn single_shr_6() -> Candidate {
    single_construction_shr(6)
}
pub fn single_shr_7() -> Candidate {
    single_construction_shr(7)
}

/// Full byte domain plus `W` independence on the same 16-value subset as
/// the right-4 spec: one chunk per residual.
fn single_hw_cases(shift: fn(u8, u32) -> u8, r: u32) -> Vec<Case> {
    let mut cases = single_cases_over(shift, r, &single_all_bytes(), &[0x00]);
    let subset = [
        0x00, 0x01, 0x07, 0x08, 0x0F, 0x10, 0x1F, 0x3A, 0x55, 0x7E, 0x80, 0xA5, 0xC3, 0xE7, 0xF0,
        0xFF,
    ];
    cases.extend(single_cases_over(shift, r, &subset, W_SAMPLE));
    cases
}

pub fn single_shl_5_cases() -> Vec<Case> {
    single_hw_cases(u8::wrapping_shl, 5)
}
pub fn single_shl_6_cases() -> Vec<Case> {
    single_hw_cases(u8::wrapping_shl, 6)
}
pub fn single_shl_7_cases() -> Vec<Case> {
    single_hw_cases(u8::wrapping_shl, 7)
}
pub fn single_shr_5_cases() -> Vec<Case> {
    single_hw_cases(u8::wrapping_shr, 5)
}
pub fn single_shr_6_cases() -> Vec<Case> {
    single_hw_cases(u8::wrapping_shr, 6)
}
pub fn single_shr_7_cases() -> Vec<Case> {
    single_hw_cases(u8::wrapping_shr, 7)
}

/// 16-bit pair cases over an explicit pair list: bit patterns at a single
/// `W` (the tests' own exhaustive convention) plus the curated `W` cross
/// separately. Shared by left and right shifts via `shift`.
fn pairs16_cases(
    shift: fn(u16, u32) -> u16,
    amount: u32,
    pairs: &[(u8, u8)],
    w_values: &[u8],
) -> Vec<Case> {
    let mut cases = Vec::new();
    for &(lo, hi) in pairs {
        for &w in w_values {
            for c in [false, true] {
                let x = (u16::from(hi) << 8) | u16::from(lo);
                let expect = shift(x, amount);
                let status = if c { STATUS_C_BIT } else { 0 };
                cases.push(Case {
                    entry_w: w,
                    pokes: vec![(LO, lo), (HI, hi), (STATUS_ADDR, status)],
                    allowed_changes: vec![LO, HI],
                    check: Box::new(move |sim: &Pic18| {
                        sim.ram()[LO] == (expect & 0xFF) as u8
                            && sim.ram()[HI] == (expect >> 8) as u8
                    }),
                });
            }
        }
    }
    cases
}

const CURATED8: [(u8, u8); 8] = [
    (0x00, 0x00),
    (0xFF, 0xFF),
    (0x0F, 0xF0),
    (0xF0, 0x0F),
    (0x01, 0x80),
    (0x12, 0x34),
    (0xAB, 0xCD),
    (0x55, 0xAA),
];

fn lcg16(seed: u32, n: usize) -> Vec<(u8, u8)> {
    let mut state = seed;
    (0..n)
        .map(|_| {
            let x = lcg_next(&mut state) as u16;
            (x as u8, (x >> 8) as u8)
        })
        .collect()
}

/// PR sample (200 pairs): curated edges, walking ones/zeros across the
/// pair, and a 160-case LCG sweep for carry/nibble interactions.
fn sample16_pr() -> Vec<(u8, u8)> {
    let mut v: Vec<(u8, u8)> = CURATED8.to_vec();
    for k in 0..16u32 {
        let x = 1u16 << k;
        v.push((x as u8, (x >> 8) as u8));
        let z = !(1u16 << k);
        v.push((z as u8, (z >> 8) as u8));
    }
    v.extend(lcg16(0x2B7E_1516, 160));
    v
}

/// Nightly sample: the PR pairs plus every byte against 0x00/0xFF in both
/// lanes and a 256-case LCG sweep. The full 65536-pair domain stays
/// sim-only (one unrolled HEX cannot hold it, and 300+ sessions would
/// make even nightly useless); executor divergence is systematic, so
/// structured edges plus randomness catch it.
fn sample16_nightly() -> Vec<(u8, u8)> {
    let mut v = sample16_pr();
    for b in 0..=255u8 {
        v.push((b, 0x00));
        v.push((b, 0xFF));
        v.push((0x00, b));
        v.push((0xFF, b));
    }
    v.extend(lcg16(0x0917_6E2B, 256));
    v
}

fn shift16_hw(shift: fn(u16, u32) -> u16, amount: u32, pr: bool) -> Vec<Case> {
    let pairs = if pr {
        sample16_pr()
    } else {
        sample16_nightly()
    };
    let mut cases = pairs16_cases(shift, amount, &pairs, &[0x00]);
    cases.extend(pairs16_cases(shift, amount, &CURATED8, W_SAMPLE));
    cases
}

/// Amount 4's construction: `SWAPF` both bytes, mask, recombine the
/// straddling nibble (moved from the spec test).
pub fn shift_16bit_4() -> Candidate {
    vec![
        "swapf 0x021,F,A",
        "movlw 0xF0",
        "andwf 0x021,F,A",
        "swapf 0x020,W,A",
        "andlw 0x0F",
        "iorwf 0x021,F,A",
        "swapf 0x020,F,A",
        "movlw 0xF0",
        "andwf 0x020,F,A",
    ]
}

/// Amount 5: amount 4 plus one standard rotate step.
pub fn shift_16bit_5() -> Candidate {
    let mut c = shift_16bit_4();
    c.push("bcf 0xFD8,0,A");
    c.push("rlcf 0x020,F,A");
    c.push("rlcf 0x021,F,A");
    c
}

/// Amount 6: the general family's own instance (`RRNCF` twice per byte is
/// a rotate-right-2, equal to rotate-left-6). Masks are `0xC0` (keep the
/// top 2 bits after rotation) and `0x3F` (the 6 straddling bits).
pub fn shift_16bit_6() -> Candidate {
    vec![
        "rrncf 0x021,F,A",
        "rrncf 0x021,F,A",
        "movlw 0xC0",
        "andwf 0x021,F,A",
        "rrncf 0x020,F,A",
        "rrncf 0x020,F,A",
        "movf 0x020,W,A",
        "andlw 0x3F",
        "iorwf 0x021,F,A",
        "movlw 0xC0",
        "andwf 0x020,F,A",
    ]
}

/// Amount 7: one bit short of a byte boundary, so a 16-bit logical
/// right-shift-by-1 (carry seeded 0) followed by a byte move turns
/// `x << 7` into 7 words: the byte landing in `LO` is the byte `HI`
/// needs, with `LO` then cleared and rotated once more for the last bit.
pub fn shift_16bit_7() -> Candidate {
    vec![
        "bcf 0xFD8,0,A",
        "rrcf 0x021,F,A",
        "rrcf 0x020,F,A",
        "movf 0x020,W,A",
        "movwf 0x021,A",
        "clrf 0x020,A",
        "rrcf 0x020,F,A",
    ]
}

/// 16-bit right shift by 4, W-only like the landed left-shift forms.
/// `lo' = (lo>>4) | ((hi&0x0F)<<4)` and `hi' = hi>>4`, each a nibble-swap
/// plus a mask. Nine words against the 12-word unroll (moved from the
/// spec test with its derivation).
pub fn shift_16bit_r4() -> Candidate {
    vec![
        "swapf 0x020,F,A",
        "movlw 0x0F",
        "andwf 0x020,F,A",
        "swapf 0x021,W,A",
        "andlw 0xF0",
        "iorwf 0x020,F,A",
        "swapf 0x021,W,A",
        "andlw 0x0F",
        "movwf 0x021,A",
    ]
}

pub fn shift_16bit_4_pr() -> Vec<Case> {
    shift16_hw(u16::wrapping_shl, 4, true)
}
pub fn shift_16bit_5_pr() -> Vec<Case> {
    shift16_hw(u16::wrapping_shl, 5, true)
}
pub fn shift_16bit_6_pr() -> Vec<Case> {
    shift16_hw(u16::wrapping_shl, 6, true)
}
pub fn shift_16bit_7_pr() -> Vec<Case> {
    shift16_hw(u16::wrapping_shl, 7, true)
}
pub fn shift_16bit_r4_pr() -> Vec<Case> {
    shift16_hw(u16::wrapping_shr, 4, true)
}
pub fn shift_16bit_4_nightly() -> Vec<Case> {
    shift16_hw(u16::wrapping_shl, 4, false)
}
pub fn shift_16bit_5_nightly() -> Vec<Case> {
    shift16_hw(u16::wrapping_shl, 5, false)
}
pub fn shift_16bit_6_nightly() -> Vec<Case> {
    shift16_hw(u16::wrapping_shl, 6, false)
}
pub fn shift_16bit_7_nightly() -> Vec<Case> {
    shift16_hw(u16::wrapping_shl, 7, false)
}
pub fn shift_16bit_r4_nightly() -> Vec<Case> {
    shift16_hw(u16::wrapping_shr, 4, false)
}

pub fn cases_with32(
    shift: fn(u32, u32) -> u32,
    amount: u32,
    words: &[[u8; 4]],
    w_values: &[u8],
) -> Vec<Case> {
    let mut cases = Vec::new();
    for &[b0, b1, b2, b3] in words {
        for &w in w_values {
            for c in [false, true] {
                let x = u32::from(b0)
                    | (u32::from(b1) << 8)
                    | (u32::from(b2) << 16)
                    | (u32::from(b3) << 24);
                let expect = shift(x, amount);
                let e = expect.to_le_bytes();
                let status = if c { STATUS_C_BIT } else { 0 };
                cases.push(Case {
                    entry_w: w,
                    pokes: vec![
                        (BASE32, b0),
                        (BASE32 + 1, b1),
                        (BASE32 + 2, b2),
                        (BASE32 + 3, b3),
                        (STATUS_ADDR, status),
                    ],
                    allowed_changes: vec![BASE32, BASE32 + 1, BASE32 + 2, BASE32 + 3],
                    check: Box::new(move |sim: &Pic18| {
                        sim.ram()[BASE32] == e[0]
                            && sim.ram()[BASE32 + 1] == e[1]
                            && sim.ram()[BASE32 + 2] == e[2]
                            && sim.ram()[BASE32 + 3] == e[3]
                    }),
                });
            }
        }
    }
    cases
}

/// The test's own wide sweep (moved from the spec test): every value in
/// each lane independently, boundary patterns, and a 4096-case LCG for
/// carry interactions across lane boundaries.
pub fn swept_words32() -> Vec<[u8; 4]> {
    let mut v: Vec<[u8; 4]> = Vec::new();
    let base = [0x12u8, 0x34, 0x56, 0x78];
    for lane in 0..4usize {
        for b in 0..=u8::MAX {
            let mut w = base;
            w[lane] = b;
            v.push(w);
        }
    }
    for w in [
        [0x00, 0x00, 0x00, 0x00],
        [0xFF, 0xFF, 0xFF, 0xFF],
        [0x01, 0x00, 0x00, 0x80],
        [0x80, 0x00, 0x00, 0x01],
        [0xAA, 0x55, 0xAA, 0x55],
    ] {
        v.push(w);
    }
    let mut state: u32 = 0x1234_5678;
    for _ in 0..4096 {
        v.push(lcg_next(&mut state).to_le_bytes());
    }
    v
}

fn lcg_words32(seed: u32, n: usize) -> Vec<[u8; 4]> {
    let mut state = seed;
    (0..n).map(|_| lcg_next(&mut state).to_le_bytes()).collect()
}

/// PR word list: lane sweep at stride 4, boundaries, and a 128-case LCG.
fn sample32_pr_words() -> Vec<[u8; 4]> {
    let mut v: Vec<[u8; 4]> = Vec::new();
    let base = [0x12u8, 0x34, 0x56, 0x78];
    for lane in 0..4usize {
        for b in (0..=255u16).step_by(4) {
            let mut w = base;
            w[lane] = b as u8;
            v.push(w);
        }
    }
    for w in [
        [0x00, 0x00, 0x00, 0x00],
        [0xFF, 0xFF, 0xFF, 0xFF],
        [0x01, 0x00, 0x00, 0x80],
        [0x80, 0x00, 0x00, 0x01],
        [0xAA, 0x55, 0xAA, 0x55],
    ] {
        v.push(w);
    }
    v.extend(lcg_words32(0xC0FF_EE11, 128));
    v
}

/// Nightly word list: structured sweep fully (every lane value) plus a
/// 1024-case LCG spot-check. The full 4096-LCG stays sim-only: hardware
/// proves executor agreement on structured coverage, sim owns bulk
/// randomness, and both lists are deterministic.
fn sample32_nightly_words() -> Vec<[u8; 4]> {
    let mut v: Vec<[u8; 4]> = Vec::new();
    let base = [0x12u8, 0x34, 0x56, 0x78];
    for lane in 0..4usize {
        for b in 0..=u8::MAX {
            let mut w = base;
            w[lane] = b;
            v.push(w);
        }
    }
    for w in [
        [0x00, 0x00, 0x00, 0x00],
        [0xFF, 0xFF, 0xFF, 0xFF],
        [0x01, 0x00, 0x00, 0x80],
        [0x80, 0x00, 0x00, 0x01],
        [0xAA, 0x55, 0xAA, 0x55],
    ] {
        v.push(w);
    }
    v.extend(lcg_words32(0xC0FF_EE11, 1024));
    v
}

/// The general 4-lane left-shift family (moved from the spec test):
/// rotate every byte right by `8 - r` with `RRNCF`, recombine high to low.
pub fn shift_32bit_family(r: u32) -> Candidate {
    let (rot, hi, lo) = match r {
        4 => (4, "movlw 0xF0", "andlw 0x0F"),
        5 => (3, "movlw 0xE0", "andlw 0x1F"),
        6 => (2, "movlw 0xC0", "andlw 0x3F"),
        7 => (1, "movlw 0x80", "andlw 0x7F"),
        _ => unreachable!("only the amounts with a fused form"),
    };
    let mut c: Candidate = Vec::new();
    for _ in 0..rot {
        for a in [
            "rrncf 0x023,F,A",
            "rrncf 0x022,F,A",
            "rrncf 0x021,F,A",
            "rrncf 0x020,F,A",
        ] {
            c.push(a);
        }
    }
    for b in (1..4usize).rev() {
        c.push(hi);
        c.push(["andwf 0x021,F,A", "andwf 0x022,F,A", "andwf 0x023,F,A"][b - 1]);
        c.push(["movf 0x020,W,A", "movf 0x021,W,A", "movf 0x022,W,A"][b - 1]);
        c.push(lo);
        c.push(["iorwf 0x021,F,A", "iorwf 0x022,F,A", "iorwf 0x023,F,A"][b - 1]);
    }
    c.push(hi);
    c.push("andwf 0x020,F,A");
    c
}

pub fn shift_32bit_6() -> Candidate {
    shift_32bit_family(6)
}
pub fn shift_32bit_7() -> Candidate {
    shift_32bit_family(7)
}

pub fn shift_32bit_6_pr() -> Vec<Case> {
    cases_with32(u32::wrapping_shl, 6, &sample32_pr_words(), &[0x00])
}
pub fn shift_32bit_7_pr() -> Vec<Case> {
    cases_with32(u32::wrapping_shl, 7, &sample32_pr_words(), &[0x00])
}
pub fn shift_32bit_6_nightly() -> Vec<Case> {
    let mut cases = cases_with32(u32::wrapping_shl, 6, &sample32_nightly_words(), &[0x00]);
    cases.extend(cases_with32(
        u32::wrapping_shl,
        6,
        &sample32_pr_words(),
        &[0xFF, 0x2A],
    ));
    cases
}
pub fn shift_32bit_7_nightly() -> Vec<Case> {
    let mut cases = cases_with32(u32::wrapping_shl, 7, &sample32_nightly_words(), &[0x00]);
    cases.extend(cases_with32(
        u32::wrapping_shl,
        7,
        &sample32_pr_words(),
        &[0xFF, 0x2A],
    ));
    cases
}

/// Bitmask lanes (epic-cc#626): the exact skip-over-`BSF` sequences
/// `isel-pic18` emits for `if (field == k) acc |= mask`, verified in-sim
/// here and replayed on hardware by the `mdb_oracle` bin. Candidates use
/// one fixed label; `mdb::build_batch` uniquifies it per case.
const LANE_FIELD: usize = 0x020;
const LANE_ACC: usize = 0x021;

/// `if (field == 1) acc |= 0x04`: `DECFSZ` skips the branch exactly on 1.
pub fn bitmask_eq1_candidate() -> Candidate {
    vec![
        "decfsz 0x020,W,A",
        "bra lane_skip",
        "bsf 0x021,2,A",
        "lane_skip:",
    ]
}

/// Inverted order (`BSF` first): the same test with the set-arm on the
/// predicate's false side.
pub fn bitmask_eq1_inv_candidate() -> Candidate {
    vec![
        "decfsz 0x020,W,A",
        "bsf 0x021,2,A",
        "bra lane_skip",
        "lane_skip:",
    ]
}

/// `if (field) acc |= 0x20`: a `MOVF` truthiness test over the branch.
pub fn bitmask_truthy_candidate() -> Candidate {
    vec![
        "movf 0x020,W,A",
        "bz lane_skip",
        "bsf 0x021,5,A",
        "lane_skip:",
    ]
}

/// Conditional base: default byte materialized first, then the test
/// selects between keeping it and setting one bit.
pub fn bitmask_base_candidate() -> Candidate {
    vec![
        "movlw 0x02",
        "movwf 0x021,A",
        "decfsz 0x020,W,A",
        "bra lane_skip",
        "bsf 0x021,4,A",
        "lane_skip:",
    ]
}

/// Lane cases over field and accumulator samples: entry `W` and `C` vary
/// because neither may leak into the result. `expect` maps
/// `(field, acc)` to the accumulator's final byte.
fn bitmask_cases_over(fields: &[u8], accs: &[u8], expect: fn(u8, u8) -> u8) -> Vec<Case> {
    let mut cases = Vec::new();
    for &f in fields {
        for &a in accs {
            for &w in W_SAMPLE {
                for c in [false, true] {
                    let e = expect(f, a);
                    let status = if c { STATUS_C_BIT } else { 0 };
                    cases.push(Case {
                        entry_w: w,
                        pokes: vec![(LANE_FIELD, f), (LANE_ACC, a), (STATUS_ADDR, status)],
                        allowed_changes: vec![LANE_ACC],
                        check: Box::new(move |sim: &Pic18| sim.ram()[LANE_ACC] == e),
                    });
                }
            }
        }
    }
    cases
}

/// Edge field values (0, 1, neighbours, sign boundary, extremes) crossed
/// with an accumulator sample per outcome class.
fn bitmask_edge_fields() -> Vec<u8> {
    vec![0x00, 0x01, 0x02, 0x7E, 0x7F, 0x80, 0x81, 0xFE, 0xFF]
}

/// Nightly adds deterministic LCG field samples to the `pr` edges: same
/// shape, wider net, still one chunk.
fn bitmask_nightly_fields() -> Vec<u8> {
    let mut state = 0x626u32;
    let mut out = Vec::new();
    for _ in 0..32 {
        out.push((lcg_next(&mut state) & 0xFF) as u8);
    }
    out
}

fn bitmask_eq1_expect(f: u8, a: u8) -> u8 {
    if f == 1 {
        a | 0x04
    } else {
        a
    }
}

fn bitmask_eq1_inv_expect(f: u8, a: u8) -> u8 {
    if f == 1 {
        a
    } else {
        a | 0x04
    }
}

fn bitmask_truthy_expect(f: u8, a: u8) -> u8 {
    if f != 0 {
        a | 0x20
    } else {
        a
    }
}

fn bitmask_base_expect(f: u8, _: u8) -> u8 {
    if f == 1 {
        0x12
    } else {
        0x02
    }
}

pub fn bitmask_eq1_pr() -> Vec<Case> {
    bitmask_cases_over(
        &bitmask_edge_fields(),
        &[0x00, 0x04, 0xFB, 0xFF],
        bitmask_eq1_expect,
    )
}

pub fn bitmask_eq1_nightly() -> Vec<Case> {
    let mut cases = bitmask_eq1_pr();
    cases.extend(bitmask_cases_over(
        &bitmask_nightly_fields(),
        &[0x00, 0xFF],
        bitmask_eq1_expect,
    ));
    cases
}

pub fn bitmask_eq1_inv_pr() -> Vec<Case> {
    bitmask_cases_over(
        &bitmask_edge_fields(),
        &[0x00, 0x04, 0xFB, 0xFF],
        bitmask_eq1_inv_expect,
    )
}

pub fn bitmask_eq1_inv_nightly() -> Vec<Case> {
    let mut cases = bitmask_eq1_inv_pr();
    cases.extend(bitmask_cases_over(
        &bitmask_nightly_fields(),
        &[0x00, 0xFF],
        bitmask_eq1_inv_expect,
    ));
    cases
}

pub fn bitmask_truthy_pr() -> Vec<Case> {
    bitmask_cases_over(
        &bitmask_edge_fields(),
        &[0x00, 0x20, 0xDF, 0xFF],
        bitmask_truthy_expect,
    )
}

pub fn bitmask_truthy_nightly() -> Vec<Case> {
    let mut cases = bitmask_truthy_pr();
    cases.extend(bitmask_cases_over(
        &bitmask_nightly_fields(),
        &[0x00, 0xFF],
        bitmask_truthy_expect,
    ));
    cases
}

pub fn bitmask_base_pr() -> Vec<Case> {
    bitmask_cases_over(&bitmask_edge_fields(), &[0x00, 0xFF], bitmask_base_expect)
}

pub fn bitmask_base_nightly() -> Vec<Case> {
    let mut cases = bitmask_base_pr();
    cases.extend(bitmask_cases_over(
        &bitmask_nightly_fields(),
        &[0x00, 0xFF],
        bitmask_base_expect,
    ));
    cases
}
