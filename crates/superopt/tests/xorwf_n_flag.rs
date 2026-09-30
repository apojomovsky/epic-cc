//! In-sim side of the `xorwf-n-flag` oracle spec (epic-cc#782): the
//! sign check reads N after `XORWF`, so sim and hardware must agree on
//! exactly when that bit sets. Same builders both sides judge.

use superopt::specs::{xorwf_n_flag_candidate, xorwf_n_flag_cases};
use superopt::verify;

#[test]
fn xorwf_sets_n_from_bit_7() {
    assert!(
        verify(&xorwf_n_flag_candidate(), &xorwf_n_flag_cases()),
        "xorwf-n-flag candidate failed its own cases in-sim"
    );
}
