//! Bitmask lanes (epic-cc#626): the exact skip-over-`BSF` sequences
//! `isel-pic18` emits, verified in-sim over the spec case lists (the
//! same lists the `mdb_oracle` bin replays on hardware, so both oracles
//! judge one domain). `verify` runs branching candidates directly: only
//! the *enumeration* alphabet is straight-line.

use superopt::specs::{
    bitmask_base_candidate, bitmask_base_pr, bitmask_eq1_candidate, bitmask_eq1_inv_candidate,
    bitmask_eq1_inv_pr, bitmask_eq1_pr, bitmask_truthy_candidate, bitmask_truthy_pr,
};
use superopt::verify;

#[test]
fn bitmask_eq1_lane_verifies() {
    assert!(verify(&bitmask_eq1_candidate(), &bitmask_eq1_pr()));
}

#[test]
fn bitmask_eq1_inv_lane_verifies() {
    assert!(verify(&bitmask_eq1_inv_candidate(), &bitmask_eq1_inv_pr()));
}

#[test]
fn bitmask_truthy_lane_verifies() {
    assert!(verify(&bitmask_truthy_candidate(), &bitmask_truthy_pr()));
}

#[test]
fn bitmask_base_lane_verifies() {
    assert!(verify(&bitmask_base_candidate(), &bitmask_base_pr()));
}
