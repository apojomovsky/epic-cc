//! Stamp the built binary with its release identity.
//!
//! The docker `release` stage passes the bundle version as the EPIC_CC_VERSION
//! build ARG, which is an environment variable of the RUN step that invokes
//! `cargo build`; dev and tag builds fall back to the crate version. The
//! stamp is what `epic-cc --version` prints, so a downstream job can name the
//! exact compiler it ran.

fn main() {
    let stamp =
        std::env::var("EPIC_CC_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=EPIC_CC_STAMP={stamp}");
    println!("cargo:rerun-if-env-changed=EPIC_CC_VERSION");
}
