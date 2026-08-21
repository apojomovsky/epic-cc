//! Library half of the driver, so the argument parser and the clang/llvm-link
//! discovery logic are unit-testable without spawning the binary.

pub mod clang_discovery;
pub mod cli;
pub mod epic_cc_h;
pub mod fosc;
pub mod prescan;
