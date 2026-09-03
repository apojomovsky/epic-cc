//! epic-cc#200: a flash/RAM size-regression suite over representative
//! programs, so isel/legalize/alloc/peephole/wholeprog_opt changes can't
//! silently grow the compiled footprint. epic-cc#193 spent a whole
//! session finding and fixing exactly this kind of drift after the fact;
//! nothing in the suite before this test would have caught it happening.
//!
//! Each `Case` compiles through the real `epic-cc` binary (the same
//! surface a user gets) and the flash word count / RAM byte count are
//! read straight off its own size report (`driver::report::render_size`,
//! already the tested, stable contract `size_map_e2e.rs` pins), not
//! reconstructed via the internal pipeline. A checked-in baseline
//! (`fixtures/size_baseline.toml`) records the last-accepted numbers per
//! case; the test fails when the measured number **exceeds** the
//! baseline. Shrinking is free, no baseline update needed. Growing
//! requires a conscious `UPDATE_SIZE_BASELINE=1 cargo test -p driver
//! --test size_regression_e2e` run to accept it (rewrites the file; diff
//! it before committing, same as reviewing any other snapshot change).
//!
//! The ladder mixes program sizes deliberately: `add.c` is near-zero, so
//! it catches boilerplate/startup regressions cheaply; the vendored
//! `hal-pic16-encoder-full` case is the large multi-driver stress case
//! that actually caught #193's regression, a small slice alone would
//! not have. See `fixtures/vendor/hal-pic16-encoder-full/PROVENANCE.md`
//! for where that fixture comes from.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BaselineEntry {
    name: String,
    device: String,
    flash_words: u32,
    ram_bytes: u32,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Baseline {
    #[serde(default)]
    entry: Vec<BaselineEntry>,
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/size_baseline.toml")
}

fn load_baseline() -> Baseline {
    let text = std::fs::read_to_string(baseline_path()).unwrap_or_default();
    toml::from_str(&text).expect("parse size_baseline.toml")
}

fn save_baseline(b: &Baseline) {
    let mut text = String::from(
        "# epic-cc#200 size-regression baseline. Do not hand-edit the\n\
         # numbers; regenerate with:\n\
         #   UPDATE_SIZE_BASELINE=1 cargo test -p driver --test size_regression_e2e\n\
         # then diff and commit deliberately.\n\n",
    );
    text.push_str(&toml::to_string_pretty(b).expect("serialize baseline"));
    std::fs::write(baseline_path(), text).expect("write size_baseline.toml");
}

/// One ladder entry: what to compile, and under which device.
struct Case {
    name: &'static str,
    device: &'static str,
    includes: Vec<PathBuf>,
    defines: Vec<&'static str>,
    inputs: Vec<PathBuf>,
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture(p: &str) -> PathBuf {
    fixtures_dir().join(p)
}

fn cases() -> Vec<Case> {
    let encoder_full = "vendor/hal-pic16-encoder-full";
    vec![
        Case {
            name: "add-16f877a",
            device: "16F877A",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("add.c")],
        },
        Case {
            name: "add-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("add.c")],
        },
        Case {
            name: "hal-pic16-blink-16f877a",
            device: "16F877A",
            includes: vec![fixture("hal-pic16")],
            defines: vec![],
            inputs: [
                "hal_pic16_config.c",
                "hal_pic16_blink.c",
                "hal_pic16_gpio.c",
                "hal_pic16_timer0.c",
                "hal_pic16_irq.c",
                "hal_pic16_dispatch.c",
                "hal_pic16_vector.c",
            ]
            .iter()
            .map(|f| fixture(&format!("hal-pic16/{f}")))
            .collect(),
        },
        Case {
            name: "hal-pic18-blink-18f4550",
            device: "18F4550",
            includes: vec![fixture("hal-pic18")],
            defines: vec![],
            inputs: [
                "hal_pic18_config.c",
                "hal_pic18_blink.c",
                "hal_pic18_gpio.c",
                "hal_pic18_timer0.c",
                "hal_pic18_irq.c",
            ]
            .iter()
            .map(|f| fixture(&format!("hal-pic18/{f}")))
            .collect(),
        },
        Case {
            name: "hal-pic16-encoder-full-16f877a",
            device: "16F877A",
            includes: [
                "pic16f87xa-hal/include/epiccc",
                "pic16f87xa-hal/include",
                "epic-common/include",
                "epic-tick/include",
                "epic-encoder/include",
                "epic-serial/include",
            ]
            .iter()
            .map(|d| fixture(&format!("{encoder_full}/{d}")))
            .collect(),
            defines: vec!["PIC16F877A", "FOSC_HZ=20000000", "__EPIC_CC__"],
            inputs: [
                "pic16f87xa-hal/src/peripherals/pic16f87xa_gpio.c",
                "pic16f87xa-hal/src/peripherals/pic16f87xa_timer0.c",
                "pic16f87xa-hal/src/peripherals/pic16f87xa_timer2.c",
                "pic16f87xa-hal/src/peripherals/pic16f87xa_ssp.c",
                "pic16f87xa-hal/src/peripherals/pic16f87xa_usart.c",
                "pic16f87xa-hal/src/core/pic16_irq.c",
                "pic16f87xa-hal/src/core/pic16f87xa_wdt_sleep.c",
                "pic16f87xa-hal/src/epiccc/pic16f87xa_wdt_sleep_epiccc.c",
                "pic16f87xa-hal/src/epiccc/pic16_isr_vector.c",
                "pic16f87xa-hal/src/epiccc/pic16_irq_dispatch_epiccc.c",
                "epic-common/src/core/epic_harness_target.c",
                "epic-tick/src/epic_tick.c",
                "epic-encoder/src/encoder.c",
                "epic-serial/src/epic_serial.c",
                "epic-encoder/examples/example_encoder.c",
                "config.c",
            ]
            .iter()
            .map(|f| fixture(&format!("{encoder_full}/{f}")))
            .collect(),
        },
    ]
}

/// Run `epic-cc` for `c` and return its size-report stderr text.
fn measure(c: &Case) -> String {
    let hex_path = std::env::temp_dir().join(format!(
        "size-regression-{}-{}.hex",
        c.name,
        std::process::id()
    ));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args(["--target", c.device]);
    for inc in &c.includes {
        cmd.arg("-I").arg(inc);
    }
    for d in &c.defines {
        cmd.arg("-D").arg(d);
    }
    cmd.arg("-o").arg(&hex_path);
    cmd.args(&c.inputs);
    let out = cmd.output().expect("run epic-cc");
    let _ = std::fs::remove_file(&hex_path);
    assert!(
        out.status.success(),
        "epic-cc {} ({}): {}",
        c.name,
        c.device,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Pull the number right before the first `/` after `marker` out of the
/// size report, e.g. `parse_after(report, "flash: ")` from a
/// `"  flash: 1234/8192 words (...)"` line.
fn parse_after(report: &str, marker: &str) -> u32 {
    let start = report
        .find(marker)
        .unwrap_or_else(|| panic!("missing {marker:?} in report:\n{report}"))
        + marker.len();
    let rest = &report[start..];
    let end = rest.find('/').expect("size report always has N/total");
    rest[..end]
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("bad number after {marker:?}: {e}\n{report}"))
}

#[test]
fn flash_and_ram_do_not_regress() {
    let update = std::env::var("UPDATE_SIZE_BASELINE").is_ok();
    let baseline = load_baseline();
    let mut measured = Vec::new();
    let mut failures = Vec::new();

    for c in cases() {
        let report = measure(&c);
        let flash_words = parse_after(&report, "flash: ");
        let ram_bytes = parse_after(&report, "RAM: ");

        if let Some(base) = baseline.entry.iter().find(|e| e.name == c.name) {
            if !update {
                if flash_words > base.flash_words {
                    failures.push(format!(
                        "{}: flash grew {} -> {} words (+{})",
                        c.name,
                        base.flash_words,
                        flash_words,
                        flash_words - base.flash_words
                    ));
                }
                if ram_bytes > base.ram_bytes {
                    failures.push(format!(
                        "{}: RAM grew {} -> {} bytes (+{})",
                        c.name,
                        base.ram_bytes,
                        ram_bytes,
                        ram_bytes - base.ram_bytes
                    ));
                }
            }
        } else if !update {
            failures.push(format!(
                "{}: no baseline entry (run with UPDATE_SIZE_BASELINE=1 to add one)",
                c.name
            ));
        }

        measured.push(BaselineEntry {
            name: c.name.to_string(),
            device: c.device.to_string(),
            flash_words,
            ram_bytes,
        });
    }

    if update {
        save_baseline(&Baseline { entry: measured });
        return;
    }

    assert!(
        failures.is_empty(),
        "size regression(s):\n{}\n\nIf intentional, re-baseline with:\n  \
         UPDATE_SIZE_BASELINE=1 cargo test -p driver --test size_regression_e2e",
        failures.join("\n")
    );
}
