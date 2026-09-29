//! Exhaustive-input equivalence for the epic-cc#750 lane reassociations:
//! every old sequence and its replacement run on `Pic14` over all 256
//! input bytes crossed with entry W and entry STATUS (Z/C/DC), and the
//! observable machine state must match. Two replacements differ by design
//! (asserted separately): `and-0` keeps W instead of zeroing it (the
//! tracker clear at the `CLRF` is load-bearing), and threading plus the
//! dead scratch store leave the consumed temp slot stale (single-use by
//! construction, flushed everywhere else). A future change to the fold
//! order or the freshness witness that breaks values or flags fails here
//! even when the shape pins still pass.
//!
//! Encodings follow the simulator's own conventions, which invert the
//! datasheet's `d` (0 routes to W in `write_d`): every helper pins an
//! absolute value (RAM, W, or STATUS) somewhere, so a mis-encoded opcode
//! fails instead of passing vacuously.

use pic14_sim::Pic14;

const S: usize = 0x20;
const D: usize = 0x21;
const STATUS: usize = 0x03;

fn movf_w(f: usize) -> u16 {
    0x0800 | f as u16
}
fn movwf(f: usize) -> u16 {
    0x0080 | f as u16
}
fn clrf(f: usize) -> u16 {
    0x0180 | f as u16
}
fn xorlw(k: u8) -> u16 {
    0x3A00 | k as u16
}
fn andlw(k: u8) -> u16 {
    0x3900 | k as u16
}
fn iorlw(k: u8) -> u16 {
    0x3800 | k as u16
}
fn iorwf_w(f: usize) -> u16 {
    0x0400 | f as u16
}

fn run(words: &[u16], s_val: u8, entry_w: u8, entry_status: u8) -> Pic14 {
    let mut p = Pic14::new(words.to_vec());
    p.ram_mut().fill(0xA5);
    p.ram_mut()[S] = s_val;
    p.ram_mut()[STATUS] = entry_status;
    p.set_w(entry_w);
    p.run(64);
    p
}

fn check_equal(old: &Pic14, new: &Pic14, ctx: &str) {
    assert_eq!(old.ram(), new.ram(), "RAM diverged ({ctx})");
    assert_eq!(old.w(), new.w(), "W diverged ({ctx})");
    assert_eq!(old.status(), new.status(), "STATUS diverged ({ctx})");
}

#[test]
fn lane_reassociations_hold_over_all_inputs() {
    let statuses = [0x00u8, 0x01, 0x02, 0x04, 0x07, 0x1F];
    let ws = [0x00u8, 0x01, 0x80, 0xFF];
    let mut cases = 0u32;
    for s in 0..=255u8 {
        for &st in &statuses {
            for &w in &ws {
                // xor-0 and ior-0 drop: value and flag identity.
                for (name, op) in [("xor", xorlw(0)), ("ior", iorlw(0))] {
                    let old = run(&[movf_w(S), op, movwf(D)], s, w, st);
                    let new = run(&[movf_w(S), movwf(D)], s, w, st);
                    check_equal(&old, &new, &format!("{name}-0 s={s:02X}"));
                    cases += 1;
                }
                // and-0 folds to CLRF: RAM and STATUS identical, W kept.
                let old = run(&[movf_w(S), andlw(0), movwf(D)], s, w, st);
                let new = run(&[clrf(D)], s, w, st);
                assert_eq!(old.ram(), new.ram(), "and-0 RAM s={s:02X}");
                assert_eq!(old.status(), new.status(), "and-0 STATUS s={s:02X}");
                assert_eq!(old.w(), 0, "and-0 old zeroes W s={s:02X}");
                assert_eq!(new.w(), w, "and-0 new preserves entry W s={s:02X}");
                cases += 1;
                // Threading: deferred store plus adopted reload vanish,
                // W and flags identical, temp slot stale by design.
                for m in [0x01u8, 0x04, 0x7F, 0x80, 0xFF] {
                    let old = run(&[movf_w(S), andlw(m), movwf(D), movf_w(D)], s, w, st);
                    let new = run(&[movf_w(S), andlw(m)], s, w, st);
                    assert_eq!(old.w(), new.w(), "thread W s={s:02X} m={m:02X}");
                    assert_eq!(
                        old.status(),
                        new.status(),
                        "thread STATUS s={s:02X} m={m:02X}"
                    );
                    cases += 1;
                }
                // Dead scratch store: the trailing MOVWF vanishes, W and
                // flags identical, scratch stale by design. The W pin also
                // proves the opcode decoded as IORWF: any impostor (a bit
                // op, a literal) would leave entry W instead of s | 0xA5.
                let old = run(&[movf_w(S), iorwf_w(D), movwf(D)], s, w, st);
                let new = run(&[movf_w(S), iorwf_w(D)], s, w, st);
                assert_eq!(new.w(), s | 0xA5, "IORWF executed s={s:02X}");
                assert_eq!(old.w(), new.w(), "dead-store W s={s:02X}");
                assert_eq!(old.status(), new.status(), "dead-store STATUS s={s:02X}");
                assert_eq!(old.ram()[D], s | 0xA5, "stored result s={s:02X}");
                assert_eq!(new.ram()[D], 0xA5, "scratch stale s={s:02X}");
                cases += 1;
            }
        }
    }
    assert!(cases > 10000, "swept the space");
}
