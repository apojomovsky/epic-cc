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
//! `SIZE_BASELINE_ONLY=a,b` scopes the rewrite to the named rows (exact
//! `Case.name` match); other rows keep their recorded values and foreign
//! drift is printed by name instead of absorbed. Every update run prints
//! which rows changed, so run with `-- --nocapture` to see the report.
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
//! Profiles (epic-cc#839): every case also compiles under `-O2`, which
//! must succeed on all rows (the speed profile builds the whole ladder).
//! Flash/RAM numbers are additionally recorded and gated for the
//! `O2_BASELINED` subset: the smallest and largest row on each core plus
//! the loop-shape bench that pins LSR under `-O2`. Gating `-O2` numbers
//! on every row would double the strict number surface future speed work
//! must re-baseline without adding signal, so the remaining rows get
//! `-O2` build coverage only (success asserted, numbers shown in the
//! step summary as build-only). Baseline entries carry `profile`; rows
//! without one are `-Os`. `STRICT_SIZE_BASELINE` applies to each gated
//! profile independently. `SIZE_BASELINE_ONLY` matches `name` for `-Os`
//! rows and `name@O2` for `-O2` rows.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BaselineEntry {
    name: String,
    device: String,
    /// The `-O` profile these numbers were measured under (`"Os"` or
    /// `"O2"`). Absent in older files, which are all `-Os`. Skipped on
    /// serialize for `-Os` so re-baselining never rewrites those rows.
    #[serde(default = "os_profile", skip_serializing_if = "is_os_profile")]
    profile: String,
    flash_words: u32,
    ram_bytes: u32,
}

fn os_profile() -> String {
    "Os".to_string()
}

fn is_os_profile(s: &String) -> bool {
    s == "Os"
}

/// Rows whose `-O2` numbers are recorded and gated, not just built: the
/// smallest and largest ladder row on each core, plus the loop-shape
/// bench that pins LSR under `-O2`. Every other row still compiles
/// under `-O2` (build coverage), without a gated number.
const O2_BASELINED: &[&str] = &[
    "add-16f877a",
    "add-18f4550",
    "bench-struct-scan-18f4550",
    "bench-struct-scan-16f877a",
    "hal-pic16-encoder-full-16f877a",
    "hal-pic18-menu-demo-18f4550",
];

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
fn parse_only_filter(valid: &[String]) -> Option<std::collections::HashSet<String>> {
    let raw = std::env::var("SIZE_BASELINE_ONLY").ok()?;
    let names: std::collections::HashSet<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if names.is_empty() {
        return None;
    }
    for n in &names {
        assert!(
            valid.iter().any(|v| v == n),
            "SIZE_BASELINE_ONLY={n} matches no size case"
        );
    }
    Some(names)
}

/// Merge measured values over the loaded baseline. Without a filter every
/// row comes from measured. With a filter only listed rows are taken from
/// measured; every other row keeps its baseline value verbatim, so a
/// stacked branch cannot absorb foreign rows. A non-filter row that shrank
/// keeps its higher baseline value on purpose: shrinking is free under the
/// gate, and the gain lands when its owner re-baselines. A new case not in
/// the filter is skipped, not added. Returns rows to save plus updated and
/// skipped report lines; updated lines carry old->new values for the PR body.
///
/// Rows are keyed by `(name, profile)`: `-Os` rows keep the bare `name`
/// key, `-O2` rows use `name@O2`, so the filter and the report tell the
/// two profiles apart.
fn entry_key(name: &str, profile: &str) -> String {
    if profile == "Os" {
        name.to_string()
    } else {
        format!("{name}@{profile}")
    }
}

fn merge_baseline(
    baseline: &Baseline,
    measured: &[BaselineEntry],
    filter: Option<&std::collections::HashSet<String>>,
) -> (Vec<BaselineEntry>, Vec<String>, Vec<String>) {
    let mut to_save = Vec::with_capacity(measured.len());
    let mut updated = Vec::new();
    let mut skipped = Vec::new();
    for m in measured {
        let key = entry_key(&m.name, &m.profile);
        let base = baseline
            .entry
            .iter()
            .find(|e| e.name == m.name && e.profile == m.profile);
        if filter.is_some_and(|f| !f.contains(&key)) {
            match base {
                Some(b) => {
                    to_save.push(b.clone());
                    if b.flash_words != m.flash_words || b.ram_bytes != m.ram_bytes {
                        skipped.push(format!(
                            "SKIPPED (not in SIZE_BASELINE_ONLY) {key}: measured flash {} RAM {}, kept baseline flash {} RAM {}",
                            m.flash_words, m.ram_bytes, b.flash_words, b.ram_bytes
                        ));
                    }
                }
                None => {
                    skipped.push(format!(
                        "SKIPPED (not in SIZE_BASELINE_ONLY) {key}: new case, not added (measured flash {} RAM {})",
                        m.flash_words, m.ram_bytes
                    ));
                }
            }
            continue;
        }
        match base {
            Some(b) if b.flash_words == m.flash_words && b.ram_bytes == m.ram_bytes => {
                to_save.push(m.clone());
            }
            Some(b) => {
                updated.push(format!(
                    "UPDATED {key}: flash {}->{} RAM {}->{}",
                    b.flash_words, m.flash_words, b.ram_bytes, m.ram_bytes
                ));
                to_save.push(m.clone());
            }
            None => {
                updated.push(format!(
                    "UPDATED {key}: new entry (flash {} RAM {})",
                    m.flash_words, m.ram_bytes
                ));
                to_save.push(m.clone());
            }
        }
    }
    (to_save, updated, skipped)
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
            name: "bench-const-sub-18f4550",
            device: "18F4550",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("size-bench/bench-const-sub.c")],
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
            // No hal-pic14e HAL slice exists in tree, so the 14E blink
            // is the xc.h tutorial blink (epic-cc#688): SFR bit ops and
            // delay loops through isel-pic14e MOVLB banking.
            name: "hal-pic14e-blink-16f1937",
            device: "16F1937",
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture("xc_blink_pic14e.c")],
        },
        Case {
            name: "hal-pic16-encoder-full-16f877a",
            device: "16F877A",
            includes: [
                "hal/pic14/16f87xa/include/epiccc",
                "hal/pic14/16f87xa/include",
                "hal/pic14/core/include",
                "common/include",
                "lib/tick/include",
                "lib/encoder/include",
                "lib/serial/include",
            ]
            .iter()
            .map(|d| fixture(&format!("{encoder_full}/{d}")))
            .collect(),
            defines: vec!["PIC16F877A", "FOSC_HZ=20000000", "__EPIC_CC__"],
            inputs: [
                "hal/pic14/core/src/peripherals/pic14_gpio.c",
                "hal/pic14/core/src/peripherals/pic14_timer0.c",
                "hal/pic14/core/src/peripherals/pic14_timer2.c",
                "hal/pic14/core/src/peripherals/pic14_ssp.c",
                "hal/pic14/core/src/peripherals/pic14_usart.c",
                "hal/pic14/core/src/core/pic14_irq.c",
                "hal/pic14/core/src/core/pic14_wdt_sleep.c",
                "hal/pic14/16f87xa/src/core/pic16_irq_table.c",
                "hal/pic14/core/src/epiccc/pic14_wdt_sleep_epiccc.c",
                "hal/pic14/core/src/epiccc/pic16_isr_vector.c",
                "hal/pic14/core/src/epiccc/pic16_irq_dispatch_epiccc.c",
                "common/src/core/epic_harness_target.c",
                "lib/tick/src/epic_tick.c",
                "lib/encoder/src/encoder.c",
                "lib/serial/src/epic_serial.c",
                "lib/encoder/examples/example_encoder.c",
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
                "vendor/hal-pic18-base/epic-math/tests",
                "vendor/hal-pic18-base/epic-pid/include",
                "vendor/hal-pic18-base/epic-adcfilter/include",
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
                "vendor/hal-pic18-base/epic-adcfilter/src/epic_adcfilter.c",
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
                "vendor/hal-pic18-base/epic-math/tests",
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
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_ssp.c",
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
        Case {
            name: "hal-pic18-bridge-demo-18f4550",
            device: "18F4550",
            includes: [
                "vendor/hal-pic18-base/pic18fxx5x-hal/include/epiccc",
                "vendor/hal-pic18-base/pic18fxx5x-hal/include",
                "vendor/hal-pic18-base/epic-common/include",
                "vendor/hal-pic18-base/epic-taskmgr/include",
                "vendor/hal-pic18-base/epic-serial/include",
                "vendor/hal-pic18-base/epic-tick/include",
                "vendor/hal-pic18-base/epic-modbus/include",
                "vendor/hal-pic18-base/epic-bus/include",
                "vendor/hal-pic18-base/epic-mcp23x17/include",
                "vendor/hal-pic18-base/epic-adcfilter/include",
                "vendor/hal-pic18-bridge-demo/epic-bridge-demo/include",
            ]
            .iter()
            .map(|d| fixture(d))
            .collect(),
            defines: vec!["PIC18F4550", "FOSC_HZ=48000000", "__EPIC_CC__"],
            inputs: [
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_gpio.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer0.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer2.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_ssp.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_usart.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_adc.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/peripherals/pic18fxx5x_eeprom.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/core/pic18_irq.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/core/pic18fxx5x_wdt_sleep.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18fxx5x_wdt_sleep_epiccc.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18_isr_vector.c",
                "vendor/hal-pic18-base/pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_epiccc_tick.c",
                "vendor/hal-pic18-bridge-demo/pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c",
                "vendor/hal-pic18-base/epic-taskmgr/src/epic_taskmgr.c",
                "vendor/hal-pic18-base/epic-serial/src/epic_serial.c",
                "vendor/hal-pic18-base/epic-tick/src/epic_tick.c",
                "vendor/hal-pic18-base/epic-modbus/src/epic_modbus.c",
                "vendor/hal-pic18-base/epic-bus/src/epic_bus.c",
                "vendor/hal-pic18-base/epic-mcp23x17/src/epic_mcp23x17.c",
                "vendor/hal-pic18-base/epic-adcfilter/src/epic_adcfilter.c",
                "vendor/hal-pic18-bridge-demo/epic-bridge-demo/src/bridge_demo_core.c",
                "vendor/hal-pic18-bridge-demo/epic-bridge-demo/tests/sim_bridge_demo.c",
                "vendor/hal-pic18-bridge-demo/config_18F4550.c",
            ]
            .iter()
            .map(|f| fixture(f))
            .collect(),
        },
    ]
}

/// Run `epic-cc` for `c` under `profile` (`"Os"` is the default flags,
/// anything else is passed as `-O<profile>`) and return its size-report
/// stderr text. A failing profile build fails the test: every profile
/// must build every ladder row.
fn measure(c: &Case, profile: &str) -> String {
    let hex_path = std::env::temp_dir().join(format!(
        "size-regression-{}-{}-{}.hex",
        c.name,
        profile,
        std::process::id()
    ));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args(["--target", c.device]);
    if profile != "Os" {
        cmd.arg(format!("-{profile}"));
    }
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
        "epic-cc {} ({} -O{}): {}",
        c.name,
        c.device,
        profile,
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

/// Gate one measured `(flash, ram)` pair against its baseline entry:
/// growth fails, and under `strict` so does headroom. Skipped entirely
/// on update runs (which record instead of asserting).
fn check_gated(
    key: &str,
    flash_words: u32,
    ram_bytes: u32,
    base: Option<&BaselineEntry>,
    update: bool,
    strict: bool,
    failures: &mut Vec<String>,
) {
    if update {
        return;
    }
    let Some(base) = base else {
        failures.push(format!(
            "{key}: no baseline entry (run with UPDATE_SIZE_BASELINE=1 to add one)"
        ));
        return;
    };
    if flash_words > base.flash_words {
        failures.push(format!(
            "{key}: flash grew {} -> {} words (+{})",
            base.flash_words,
            flash_words,
            flash_words - base.flash_words
        ));
    }
    if ram_bytes > base.ram_bytes {
        failures.push(format!(
            "{key}: RAM grew {} -> {} bytes (+{})",
            base.ram_bytes,
            ram_bytes,
            ram_bytes - base.ram_bytes
        ));
    }
    if strict {
        if let Some(msg) = drift_report(key, "flash", base.flash_words, flash_words) {
            failures.push(msg);
        }
        if let Some(msg) = drift_report(key, "RAM", base.ram_bytes, ram_bytes) {
            failures.push(msg);
        }
    }
}

#[test]
fn flash_and_ram_do_not_regress() {
    let update = std::env::var("UPDATE_SIZE_BASELINE").is_ok();
    // Strict mode turns a stale baseline row into a failure. CI sets it so
    // headroom cannot accumulate unnoticed; the default stays permissive so
    // a perf change may shrink a program without re-baselining mid-iteration.
    let strict = std::env::var("STRICT_SIZE_BASELINE").is_ok();
    let baseline = load_baseline();
    let mut measured_os = Vec::new();
    let mut measured_o2 = Vec::new();
    let mut rows = Vec::new();
    let mut failures = Vec::new();

    let cases = cases();
    // Validate the filter before measuring: a typo must fail fast, not
    // after minutes of compiling every case. `-O2` rows filter as
    // `name@O2`, `-Os` rows as the bare name.
    let mut valid: Vec<String> = cases.iter().map(|c| c.name.to_string()).collect();
    for c in &cases {
        valid.push(entry_key(c.name, "O2"));
    }
    let filter = update.then(|| parse_only_filter(&valid)).flatten();
    for c in &cases {
        // `-Os`: today's gate, unchanged semantics.
        let report = measure(c, "Os");
        let flash_words = parse_after(&report, "flash: ");
        let ram_bytes = parse_after(&report, "RAM: ");
        let base = baseline
            .entry
            .iter()
            .find(|e| e.name == c.name && e.profile == "Os")
            .cloned();
        check_gated(
            c.name,
            flash_words,
            ram_bytes,
            base.as_ref(),
            update,
            strict,
            &mut failures,
        );
        rows.push(Row {
            name: c.name.to_string(),
            device: c.device,
            flash_words,
            flash_baseline: base.as_ref().map(|b| b.flash_words),
            ram_bytes,
            ram_baseline: base.as_ref().map(|b| b.ram_bytes),
            gated: true,
        });
        measured_os.push(BaselineEntry {
            name: c.name.to_string(),
            device: c.device.to_string(),
            profile: "Os".to_string(),
            flash_words,
            ram_bytes,
        });

        // `-O2`: every row must build; numbers gate only the baselined
        // subset (plus any row that already carries an `-O2` entry, so a
        // recorded number is never silently ignored).
        let report = measure(c, "O2");
        let flash_words = parse_after(&report, "flash: ");
        let ram_bytes = parse_after(&report, "RAM: ");
        let base = baseline
            .entry
            .iter()
            .find(|e| e.name == c.name && e.profile == "O2")
            .cloned();
        let key = entry_key(c.name, "O2");
        let gated = O2_BASELINED.contains(&c.name) || base.is_some();
        if gated {
            check_gated(
                &key,
                flash_words,
                ram_bytes,
                base.as_ref(),
                update,
                strict,
                &mut failures,
            );
            measured_o2.push(BaselineEntry {
                name: c.name.to_string(),
                device: c.device.to_string(),
                profile: "O2".to_string(),
                flash_words,
                ram_bytes,
            });
        }
        rows.push(Row {
            name: key,
            device: c.device,
            flash_words,
            flash_baseline: base.as_ref().map(|b| b.flash_words),
            ram_bytes,
            ram_baseline: base.as_ref().map(|b| b.ram_bytes),
            gated,
        });
    }
    write_step_summary(&rows);
    if std::env::var("SIZE_REPORT_JSON").is_ok() {
        print_report_json(&measured_os, &cases);
        return;
    }

    let mut measured = measured_os;
    measured.extend(measured_o2);
    if update {
        let (to_save, updated, skipped) = merge_baseline(&baseline, &measured, filter.as_ref());
        save_baseline(&Baseline { entry: to_save });
        if updated.is_empty() && skipped.is_empty() {
            println!("size baseline: no changes");
        }
        for line in &updated {
            println!("size baseline: {line}");
        }
        for line in &skipped {
            println!("size baseline: {line}");
        }
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
/// one), or for an `-O2` row outside the baselined subset (`gated` false),
/// where the build is covered but the number is not.
struct Row {
    name: String,
    device: &'static str,
    flash_words: u32,
    flash_baseline: Option<u32>,
    ram_bytes: u32,
    ram_baseline: Option<u32>,
    gated: bool,
}

fn signed_delta(current: u32, baseline: Option<u32>, gated: bool) -> String {
    match baseline {
        None if gated => "(new)".to_string(),
        None => "build only".to_string(),
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
            signed_delta(r.flash_words, r.flash_baseline, r.gated),
            r.ram_bytes,
            r.ram_baseline
                .map(|b| format!(" / {b}"))
                .unwrap_or_default(),
            signed_delta(r.ram_bytes, r.ram_baseline, r.gated),
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

#[test]
fn merge_baseline_scoped_filter_keeps_foreign_rows() {
    let baseline = Baseline {
        entry: vec![
            BaselineEntry {
                name: "a".into(),
                device: "d".into(),
                profile: "Os".into(),
                flash_words: 100,
                ram_bytes: 10,
            },
            BaselineEntry {
                name: "b".into(),
                device: "d".into(),
                flash_words: 200,
                profile: "Os".into(),
                ram_bytes: 20,
            },
        ],
    };
    let measured = vec![
        BaselineEntry {
            name: "a".into(),
            device: "d".into(),
            flash_words: 110,
            profile: "Os".into(),
            ram_bytes: 10,
        },
        BaselineEntry {
            name: "b".into(),
            device: "d".into(),
            flash_words: 190,
            ram_bytes: 20,
            profile: "Os".into(),
        },
    ];
    let filter: std::collections::HashSet<String> = ["a".to_string()].into_iter().collect();
    let (saved, updated, skipped) = merge_baseline(&baseline, &measured, Some(&filter));
    assert_eq!(
        saved[0].flash_words, 110,
        "filtered row takes the measured value"
    );
    assert_eq!(
        saved[1].flash_words, 200,
        "foreign shrink is kept at baseline, the gain lands with its owner"
    );
    assert!(
        updated
            .iter()
            .any(|l| l.contains("a") && l.contains("100->110")),
        "updated rows quote old->new for the PR body"
    );
    assert!(
        skipped.iter().any(|l| l.contains('b')),
        "foreign drift is reported by name"
    );
}

#[test]
fn merge_baseline_skips_new_case_outside_filter() {
    let baseline = Baseline { entry: vec![] };
    let measured = vec![BaselineEntry {
        name: "new-case".into(),
        device: "d".into(),
        flash_words: 50,
        profile: "Os".into(),
        ram_bytes: 5,
    }];
    let filter: std::collections::HashSet<String> = ["other".to_string()].into_iter().collect();
    let (saved, _, skipped) = merge_baseline(&baseline, &measured, Some(&filter));
    assert!(
        saved.is_empty(),
        "a new case outside the filter must not be added"
    );
    assert!(
        skipped.iter().any(|l| l.contains("new-case")),
        "the skipped new case is reported by name"
    );
}

#[test]
fn merge_baseline_without_filter_rewrites_changed_rows() {
    let baseline = Baseline {
        entry: vec![BaselineEntry {
            name: "a".into(),
            device: "d".into(),
            flash_words: 100,
            profile: "Os".into(),
            ram_bytes: 10,
        }],
    };
    let measured = vec![BaselineEntry {
        name: "a".into(),
        device: "d".into(),
        flash_words: 105,
        profile: "Os".into(),
        ram_bytes: 10,
    }];
    let (saved, updated, skipped) = merge_baseline(&baseline, &measured, None);
    assert_eq!(saved[0].flash_words, 105);
    assert_eq!(updated.len(), 1);
    assert!(skipped.is_empty());
}

#[test]
fn merge_baseline_keys_o2_rows_apart_from_os_rows() {
    let baseline = Baseline {
        entry: vec![BaselineEntry {
            name: "a".into(),
            device: "d".into(),
            profile: "Os".into(),
            flash_words: 100,
            ram_bytes: 10,
        }],
    };
    let measured = vec![
        BaselineEntry {
            name: "a".into(),
            device: "d".into(),
            profile: "Os".into(),
            flash_words: 100,
            ram_bytes: 10,
        },
        BaselineEntry {
            name: "a".into(),
            device: "d".into(),
            profile: "O2".into(),
            flash_words: 120,
            ram_bytes: 12,
        },
    ];
    let (saved, updated, _) = merge_baseline(&baseline, &measured, None);
    assert_eq!(saved.len(), 2, "both profiles survive the merge");
    assert_eq!(updated.len(), 1, "only the new O2 row is an update");
    assert!(
        updated[0].contains("a@O2"),
        "the O2 row reports under its profile key: {}",
        updated[0]
    );
}
