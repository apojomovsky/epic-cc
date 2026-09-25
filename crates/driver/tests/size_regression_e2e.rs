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
//! `SIZE_REPORT_JSON=1` turns the measurement pass into a machine-readable
//! dump (one JSON array between marker lines, needs `-- --nocapture`) for
//! `make size-report`. It prints and returns before any assertion, so a
//! report never fails the gate and the gate never shapes the report.
//!
//! Because shrinking is free by default, a baseline row can sit above
//! what the tree produces and silently absorb a later regression of that
//! size. `STRICT_SIZE_BASELINE=1` (set in CI) fails on such a row instead,
//! naming the headroom; see epic-cc#629. That rewrite regenerates every
//! row, so a re-baseline must be diffed row by row: absorbing a row the
//! change did not affect is how headroom accumulates in the first place.
//!
//! The ladder mixes program sizes deliberately: `add.c` is near-zero, so
//! it catches boilerplate/startup regressions cheaply; the vendored
//! `hal-pic16-encoder-full` case is the large multi-driver stress case
//! that actually caught #193's regression, a small slice alone would
//! not have, and the vendored `hal-pic18-menu-demo` case (epic-cc#469)
//! is the PIC18 counterpart, the largest whole-program PIC18 entry.
//! See each fixture's `PROVENANCE.md` for where it comes from.

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
            name: "bench-shift-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-shift.c")],
        },
        Case {
            name: "bench-wide-const-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-wide-const.c")],
        },
        Case {
            name: "bench-zero-init-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-zero-init.c")],
        },
        Case {
            name: "bench-struct-copy-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-struct-copy.c")],
        },
        Case {
            name: "bench-switch-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-switch.c")],
        },
        Case {
            name: "bench-bool-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-bool.c")],
        },
        Case {
            name: "bench-dead-store-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-dead-store.c")],
        },
        Case {
            name: "bench-w-roundtrip-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-w-roundtrip.c")],
        },
        Case {
            name: "bench-bank-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-bank.c")],
        },
        Case {
            name: "bench-handle-init-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-handle-init.c")],
        },
        Case {
            name: "bench-switch-calls-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-switch-calls.c")],
        },
        Case {
            name: "bench-u32-loop-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-u32-loop.c")],
        },
        Case {
            name: "bench-u16-dec-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-u16-dec.c")],
        },
        Case {
            name: "bench-struct-scan-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-struct-scan.c")],
        },
        Case {
            // The same walk on the 14-bit core, which has no hardware
            // multiply: the non-power-of-two stride is a shift-add chain
            // re-emitted per access, so hoisting it out of the loop is
            // worth far more here than the PIC18 row above (epic-cc#647).
            name: "bench-struct-scan-16f877a",
            device: "16F877A",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-struct-scan.c")],
        },
        Case {
            // Byte-indexed static arrays for the `LFSR` + `PLUSW0`
            // lowering with a resident pointer (epic-cc#665).
            name: "bench-plusw-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-plusw.c")],
        },
        Case {
            // Branch on a just-computed byte with no reload (epic-cc#668).
            name: "bench-branch-computed-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-branch-computed.c")],
        },
        Case {
            name: "bench-bitmask-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-bitmask.c")],
        },
        Case {
            name: "bench-hoist-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-hoist.c")],
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
        Case {
            name: "hal-pic18-menu-demo-18f4550",
            device: "18F4550",
            includes: [
                "vendor/hal-pic18-base/pic18fxx5x-hal/include/epiccc",
                "vendor/hal-pic18-base/pic18fxx5x-hal/include",
                "vendor/hal-pic18-base/epic-common/include",
                "vendor/hal-pic18-base/epic-taskmgr/include",
                "vendor/hal-pic18-base/epic-tick/include",
                "vendor/hal-pic18-menu-demo/epic-lcd/include",
                "vendor/hal-pic18-base/epic-serial/include",
                "vendor/hal-pic18-menu-demo/epic-menu-demo/include",
            ]
            .iter()
            .map(|d| fixture(d))
            .collect(),
            defines: vec!["PIC18F4550", "FOSC_HZ=48000000", "__EPIC_CC__"],
            inputs: [
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_gpio.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer0.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer2.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_usart.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_adc.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_ccp.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_eeprom.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/core/pic18_irq.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/core/pic18fxx5x_wdt_sleep.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18fxx5x_wdt_sleep_epiccc.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18_isr_vector.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_epiccc_tick.c",
                "vendor/hal-pic18-menu-demo/pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c",
                "vendor/hal-pic18-base/epic-taskmgr/src/epic_taskmgr.c",
                "vendor/hal-pic18-base/epic-tick/src/epic_tick.c",
                "vendor/hal-pic18-menu-demo/epic-lcd/src/epic_lcd.c",
                "vendor/hal-pic18-menu-demo/epic-lcd/src/epic_lcd_gpio4.c",
                "vendor/hal-pic18-base/epic-serial/src/epic_serial.c",
                "vendor/hal-pic18-menu-demo/epic-menu-demo/src/menu_demo_core.c",
                "vendor/hal-pic18-menu-demo/epic-menu-demo/tests/sim_menu_demo.c",
                "vendor/hal-pic18-menu-demo/config_18F4550.c",
            ]
            .iter()
            .map(|f| fixture(f))
            .collect(),
        },
        Case {
            name: "hal-pic18-control-demo-18f4550",
            device: "18F4550",
            includes: [
                "vendor/hal-pic18-base/pic18fxx5x-hal/include/epiccc",
                "vendor/hal-pic18-base/pic18fxx5x-hal/include",
                "vendor/hal-pic18-base/epic-common/include",
                "vendor/hal-pic18-base/epic-taskmgr/include",
                "vendor/hal-pic18-base/epic-math/include",
                "vendor/hal-pic18-base/epic-pid/include",
                "vendor/hal-pic18-control-demo/epic-adcfilter/include",
                "vendor/hal-pic18-base/epic-serial/include",
                "vendor/hal-pic18-control-demo/epic-control-demo/include",
            ]
            .iter()
            .map(|d| fixture(d))
            .collect(),
            defines: vec!["PIC18F4550", "FOSC_HZ=48000000", "__EPIC_CC__"],
            inputs: [
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_gpio.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer0.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer2.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_usart.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_adc.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_ccp.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_eeprom.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/core/pic18_irq.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/core/pic18fxx5x_wdt_sleep.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18fxx5x_wdt_sleep_epiccc.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18_isr_vector.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_epiccc_tick.c",
                "vendor/hal-pic18-control-demo/pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c",
                "vendor/hal-pic18-base/epic-taskmgr/src/epic_taskmgr.c",
                "vendor/hal-pic18-base/epic-math/src/common/epic_math_numeric.c",
                "vendor/hal-pic18-base/epic-math/src/common/epic_math_rand.c",
                "vendor/hal-pic18-base/epic-math/src/common/epic_math_sqrt.c",
                "vendor/hal-pic18-base/epic-math/src/pic18/epic_math_addsub.c",
                "vendor/hal-pic18-base/epic-math/src/pic18/epic_math_bcd.c",
                "vendor/hal-pic18-base/epic-math/src/pic18/epic_math_div.c",
                "vendor/hal-pic18-base/epic-math/src/pic18/epic_math_mul.c",
                "vendor/hal-pic18-base/epic-pid/src/pid.c",
                "vendor/hal-pic18-control-demo/epic-adcfilter/src/epic_adcfilter.c",
                "vendor/hal-pic18-base/epic-serial/src/epic_serial.c",
                "vendor/hal-pic18-control-demo/epic-control-demo/src/control_demo_core.c",
                "vendor/hal-pic18-control-demo/epic-control-demo/tests/sim_control_demo.c",
                "vendor/hal-pic18-control-demo/config_18F4550.c",
            ]
            .iter()
            .map(|f| fixture(f))
            .collect(),
        },
        Case {
            name: "hal-pic18-pid-18f4550",
            device: "18F4550",
            includes: [
                "vendor/hal-pic18-base/pic18fxx5x-hal/include/epiccc",
                "vendor/hal-pic18-base/pic18fxx5x-hal/include",
                "vendor/hal-pic18-base/epic-common/include",
                "vendor/hal-pic18-base/epic-math/include",
                "vendor/hal-pic18-base/epic-pid/include",
                "vendor/hal-pic18-base/epic-tick/include",
                "vendor/hal-pic18-base/epic-serial/include",
            ]
            .iter()
            .map(|d| fixture(d))
            .collect(),
            defines: vec!["PIC18F4550", "FOSC_HZ=48000000", "__EPIC_CC__"],
            inputs: [
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_gpio.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer0.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer2.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_usart.c",
                "vendor/hal-pic18-pid/pic18fxx5x-hal/src/peripherals/pic18fxx5x_ssp.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_eeprom.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/core/pic18_irq.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/core/pic18fxx5x_wdt_sleep.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18fxx5x_wdt_sleep_epiccc.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18_isr_vector.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_epiccc_tick.c",
                "vendor/hal-pic18-pid/pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c",
                "vendor/hal-pic18-base/epic-math/src/common/epic_math_numeric.c",
                "vendor/hal-pic18-base/epic-math/src/common/epic_math_rand.c",
                "vendor/hal-pic18-base/epic-math/src/common/epic_math_sqrt.c",
                "vendor/hal-pic18-base/epic-math/src/pic18/epic_math_addsub.c",
                "vendor/hal-pic18-base/epic-math/src/pic18/epic_math_bcd.c",
                "vendor/hal-pic18-base/epic-math/src/pic18/epic_math_div.c",
                "vendor/hal-pic18-base/epic-math/src/pic18/epic_math_mul.c",
                "vendor/hal-pic18-base/epic-pid/src/pid.c",
                "vendor/hal-pic18-base/epic-tick/src/epic_tick.c",
                "vendor/hal-pic18-base/epic-serial/src/epic_serial.c",
                "vendor/hal-pic18-pid/epic-pid/tests/sim_pid.c",
                "vendor/hal-pic18-pid/config_18F4550.c",
            ]
            .iter()
            .map(|f| fixture(f))
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

/// A baseline row that records more than the tree produces, so the ladder
/// would absorb a regression of up to `headroom` before noticing. Reported
/// (not fatal) unless the run is strict.
fn drift_report(name: &str, metric: &str, baseline: u32, measured: u32) -> Option<String> {
    if baseline <= measured {
        return None;
    }
    let headroom = baseline - measured;
    Some(format!(
        "{name}: {metric} baseline {baseline} is {headroom} above measured {measured}; \
         a regression of up to {headroom} would go undetected. Re-baseline with \
         UPDATE_SIZE_BASELINE=1 and diff the result."
    ))
}
/// Machine-readable dump of this run's measurements for `make size-report`.
///
/// `includes`/`inputs` are manifest-relative, so the report runner can
/// rebuild a listing (the menu-demo cluster table) without duplicating
/// the `cases()` file lists. Formatting is by hand: every field is
/// path-safe, so no escaping is needed, and this avoids a serde_json
/// dev-dependency for one aid.
fn rel(path: &std::path::Path) -> String {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    path.strip_prefix(manifest)
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

fn print_report_json(measured: &[BaselineEntry], cases: &[Case]) {
    println!("SIZE_REPORT_JSON_BEGIN");
    println!("[");
    for (i, e) in measured.iter().enumerate() {
        let c = &cases[i];
        let list = |ps: &[std::path::PathBuf]| {
            ps.iter()
                .map(|p| format!("\"{}\"", rel(p)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "  {{\"name\": \"{}\", \"device\": \"{}\", \"flash_words\": {}, \"ram_bytes\": {}, \"defines\": [{}], \"includes\": [{}], \"inputs\": [{}]}}{}",
            e.name,
            e.device,
            e.flash_words,
            e.ram_bytes,
            c.defines
                .iter()
                .map(|d| format!("\"{d}\""))
                .collect::<Vec<_>>()
                .join(", "),
            list(&c.includes),
            list(&c.inputs),
            if i + 1 == measured.len() { "" } else { "," }
        );
    }
    println!("]");
    println!("SIZE_REPORT_JSON_END");
}

#[test]
fn flash_and_ram_do_not_regress() {
    let update = std::env::var("UPDATE_SIZE_BASELINE").is_ok();
    // Strict mode turns a stale baseline row into a failure. CI sets it so
    // headroom cannot accumulate unnoticed; the default stays permissive so
    // a perf change may shrink a program without re-baselining mid-iteration.
    let strict = std::env::var("STRICT_SIZE_BASELINE").is_ok();
    let baseline = load_baseline();
    let mut measured = Vec::new();
    let mut rows = Vec::new();
    let mut failures = Vec::new();

    let cases = cases();
    for c in &cases {
        let report = measure(c);
        let flash_words = parse_after(&report, "flash: ");
        let ram_bytes = parse_after(&report, "RAM: ");
        let base = baseline.entry.iter().find(|e| e.name == c.name).cloned();

        if let Some(base) = &base {
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
                if strict {
                    if let Some(msg) = drift_report(c.name, "flash", base.flash_words, flash_words)
                    {
                        failures.push(msg);
                    }
                    if let Some(msg) = drift_report(c.name, "RAM", base.ram_bytes, ram_bytes) {
                        failures.push(msg);
                    }
                }
            }
        } else if !update {
            failures.push(format!(
                "{}: no baseline entry (run with UPDATE_SIZE_BASELINE=1 to add one)",
                c.name
            ));
        }

        rows.push(Row {
            name: c.name,
            device: c.device,
            flash_words,
            flash_baseline: base.as_ref().map(|b| b.flash_words),
            ram_bytes,
            ram_baseline: base.as_ref().map(|b| b.ram_bytes),
        });
        measured.push(BaselineEntry {
            name: c.name.to_string(),
            device: c.device.to_string(),
            flash_words,
            ram_bytes,
        });
    }
    write_step_summary(&rows);
    if std::env::var("SIZE_REPORT_JSON").is_ok() {
        print_report_json(&measured, &cases);
        return;
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

/// One ladder entry's measured-vs-baseline numbers, for the step-summary
/// table. `*_baseline` is `None` for a case with no recorded baseline yet
/// (only possible with `UPDATE_SIZE_BASELINE=1`, which is about to add
/// one).
struct Row {
    name: &'static str,
    device: &'static str,
    flash_words: u32,
    flash_baseline: Option<u32>,
    ram_bytes: u32,
    ram_baseline: Option<u32>,
}

fn signed_delta(current: u32, baseline: Option<u32>) -> String {
    match baseline {
        None => "(new)".to_string(),
        Some(b) => {
            let d = current as i64 - b as i64;
            if d > 0 {
                format!("+{d}")
            } else {
                d.to_string()
            }
        }
    }
}

/// Append a markdown table of every case's measured size against its
/// baseline to `$GITHUB_STEP_SUMMARY`, when set (CI only, a no-op for a
/// local `cargo test`). Runs unconditionally, on a pass or a failure, so
/// a PR shows exactly what moved and by how much instead of a bare
/// crate-level PASS/FAIL (epic-cc#200 follow-up).
fn write_step_summary(rows: &[Row]) {
    let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY") else {
        return;
    };
    let mut out = String::from(
        "\n## Size regression (epic-cc#200)\n\n\
         | Fixture | Device | Flash words | delta | RAM bytes | delta |\n\
         |---|---|---|---|---|---|\n",
    );
    for r in rows {
        out.push_str(&format!(
            "| {} | {} | {}{} | {} | {}{} | {} |\n",
            r.name,
            r.device,
            r.flash_words,
            r.flash_baseline
                .map(|b| format!(" / {b}"))
                .unwrap_or_default(),
            signed_delta(r.flash_words, r.flash_baseline),
            r.ram_bytes,
            r.ram_baseline
                .map(|b| format!(" / {b}"))
                .unwrap_or_default(),
            signed_delta(r.ram_bytes, r.ram_baseline),
        ));
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open GITHUB_STEP_SUMMARY");
    f.write_all(out.as_bytes())
        .expect("write GITHUB_STEP_SUMMARY");
}

/// The strict-mode verdict is pure arithmetic on the two numbers, so it is
/// tested without compiling anything: equal or ahead is no finding, behind
/// is one naming the headroom a regression could hide in.
#[test]
fn drift_report_flags_a_baseline_above_the_measured_value() {
    assert_eq!(
        drift_report("x", "flash", 100, 100),
        None,
        "an exact match has no headroom to hide a regression in"
    );
    assert_eq!(
        drift_report("x", "flash", 100, 120),
        None,
        "a baseline below the measured value is a growth, reported elsewhere"
    );
    // The headroom must be a number that appears nowhere else in the
    // message, or asserting on it proves nothing: a headroom of 9 against
    // measured 196 satisfies `contains('9')` even if the headroom is never
    // printed. 12 against measured 188 keeps it distinct, and the phrase
    // pins where it lands (the message names the headroom twice, in "is 12
    // above" and "up to 12", so counting occurrences would be brittle).
    let msg =
        drift_report("bench-x", "flash", 200, 188).expect("12 words of headroom must be reported");
    assert!(
        msg.contains("bench-x")
            && msg.contains("200")
            && msg.contains("188")
            && msg.contains("is 12 above"),
        "the message must name the row, both numbers and the headroom: {msg}"
    );
}
