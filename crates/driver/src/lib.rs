//! Library half of the driver, so the argument parser and the clang/llvm-link
//! discovery logic are unit-testable without spawning the binary.

pub mod clang;
pub mod clang_discovery;
pub mod cli;
pub mod epic_cc_h;
pub mod fosc;
pub mod predef;
pub mod prescan;
pub mod report;
pub mod stdbool_h;
pub mod stddef_h;
pub mod stdint_h;
pub mod stdio_h;
pub mod stdlib_h;
pub mod string_c;
pub mod string_h;
pub mod xc_h;
