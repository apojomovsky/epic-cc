//! Lane B (docs/36, #286): coverage-guided fuzz target for `irparse`'s
//! parser over the canonical IR dialect. The parser sits directly behind
//! the `.ll`-to-IR boundary the whole backend trusts, so a crash here is a
//! front-end bug worth finding. The fuzzer mutates bytes under coverage
//! feedback, so it exercises structurally broken input the differential
//! generator never produces.
//!
//! The target is deliberately a no-assert harness: `parse_ll` panics
//! loudly on malformed input (the project's contract), and libFuzzer
//! treats any panic as a crash to minimize and report. We do not catch
//! panics here; the fuzzer's job is to find them.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = irparse::parse_ll(s);
    }
});
