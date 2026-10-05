//! epic-cc#840: a cycle-count ladder over representative kernels, so
//! isel/legalize/alloc/peephole/wholeprog_opt changes can't silently
//! slow the generated code. It mirrors `size_regression_e2e.rs`: each
//! case compiles through the real `epic-cc` binary, flash/RAM come off
//! its size report, and cycles come out of `crates/sim` (which counts
//! since #700) stepped between the fixture's marker stores.
//!
//! Markers (`fixtures/speed-bench/PROVENANCE.md`): every kernel defines
//! `volatile unsigned char bench_mark`, main stores `1` before the work
//! and `2` after, and the harness resolves the address from `--map` and
//! steps the sim from one store to the other. The stores are plain C so
//! the private comparison times the identical binary shape in MPLAB SIM
//! off the same MAP file. Inputs are volatile globals loaded inside the
//! region and results sink to a volatile before the `2`, so the work is
//! data-pinned between the marks under every profile.
//!
//! ISR cases (`speed-isr-*`) measure differently: main spins in a
//! `bench_sync` window while the harness injects the interrupt with
//! `fire_interrupt`, no hardcoded PCs. Latency is vector entry to the
//! ISR's first user store; the round trip is the halt-cycle delta of an
//! interrupted run against a clean baseline run of the same binary.
//!
//! A checked-in baseline (`fixtures/cycle_baseline.toml`) records the
//! last-accepted cycles plus flash/RAM per case; the test fails when a
//! measured number **exceeds** its baseline. Shrinking is free.
//! Regenerate with `UPDATE_CYCLE_BASELINE=1 cargo test -p driver --test
//! cycle_ladder_e2e` and diff before committing.
//! `CYCLE_BASELINE_ONLY=a,b@O2` scopes the rewrite to named rows (`name`
//! for `-Os`, `name@O2` for `-O2`). `STRICT_CYCLE_BASELINE=1` (set in
//! CI) additionally fails headroom, like the size ladder's strict mode.
//! `SPEED_REPORT_JSON=1` prints the machine-readable dump (one JSON array
//! between marker lines, needs `-- --nocapture`) carrying cycles plus
//! flash words per kernel for tooling, and returns before asserting, so
//! a report never fails the gate and the gate never shapes the report.
//!
//! Every row is recorded under both `-Os` (default flags) and `-O2` (the
//! speed profile from #839, passed as `-O2`): the coordination clause on
//! #840. Baseline entries carry `profile`; rows without one are `-Os`.

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
    cycles: u64,
    flash_words: u32,
    ram_bytes: u32,
}

fn os_profile() -> String {
    "Os".to_string()
}

fn is_os_profile(s: &String) -> bool {
    s == "Os"
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Baseline {
    #[serde(default)]
    entry: Vec<BaselineEntry>,
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cycle_baseline.toml")
}

fn load_baseline() -> Baseline {
    let text = std::fs::read_to_string(baseline_path()).unwrap_or_default();
    toml::from_str(&text).expect("parse cycle_baseline.toml")
}

fn save_baseline(b: &Baseline) {
    let mut text = String::from(
        "# epic-cc#840 cycle-count baseline. Do not hand-edit the\n\
         # numbers; regenerate with:\n\
         #   UPDATE_CYCLE_BASELINE=1 cargo test -p driver --test cycle_ladder_e2e\n\
         # then diff and commit deliberately.\n\n",
    );
    text.push_str(&toml::to_string_pretty(b).expect("serialize baseline"));
    std::fs::write(baseline_path(), text).expect("write cycle_baseline.toml");
}

fn parse_only_filter(valid: &[String]) -> Option<std::collections::HashSet<String>> {
    let raw = std::env::var("CYCLE_BASELINE_ONLY").ok()?;
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
            "CYCLE_BASELINE_ONLY={n} matches no cycle case"
        );
    }
    Some(names)
}

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

/// Merge measured values over the loaded baseline. Without a filter every
/// row comes from measured. With a filter only listed rows are taken from
/// measured; every other row keeps its baseline value verbatim, so a
/// stacked branch cannot absorb foreign rows. A new case not in the
/// filter is skipped, not added. Returns rows to save plus updated and
/// skipped report lines; updated lines carry old->new values for the PR body.
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
                    if b.cycles != m.cycles
                        || b.flash_words != m.flash_words
                        || b.ram_bytes != m.ram_bytes
                    {
                        skipped.push(format!(
                            "SKIPPED (not in CYCLE_BASELINE_ONLY) {key}: measured cycles {} flash {} RAM {}, kept baseline cycles {} flash {} RAM {}",
                            m.cycles, m.flash_words, m.ram_bytes,
                            b.cycles, b.flash_words, b.ram_bytes
                        ));
                    }
                }
                None => {
                    skipped.push(format!(
                        "SKIPPED (not in CYCLE_BASELINE_ONLY) {key}: new case, not added (measured cycles {} flash {} RAM {})",
                        m.cycles, m.flash_words, m.ram_bytes
                    ));
                }
            }
            continue;
        }
        match base {
            Some(b)
                if b.cycles == m.cycles
                    && b.flash_words == m.flash_words
                    && b.ram_bytes == m.ram_bytes =>
            {
                to_save.push(m.clone());
            }
            Some(b) => {
                updated.push(format!(
                    "UPDATED {key}: cycles {}->{} flash {}->{} RAM {}->{}",
                    b.cycles, m.cycles, b.flash_words, m.flash_words, b.ram_bytes, m.ram_bytes
                ));
                to_save.push(m.clone());
            }
            None => {
                updated.push(format!(
                    "UPDATED {key}: new entry (cycles {} flash {} RAM {})",
                    m.cycles, m.flash_words, m.ram_bytes
                ));
                to_save.push(m.clone());
            }
        }
    }
    (to_save, updated, skipped)
}

/// How a case is timed: `Marks` steps the sim from the `bench_mark` 1
/// store to the 2 store; `Isr` injects an interrupt in the `bench_sync`
/// window and records latency plus round trip.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Marks,
    Isr,
}

/// One ladder entry: what to compile, and how to time it.
struct Case {
    name: String,
    device: &'static str,
    kind: Kind,
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

fn kernel(stem: &str, device: &'static str, file: &str) -> Case {
    let suffix = if device == "18F4550" {
        "18f4550"
    } else {
        "16f877a"
    };
    Case {
        name: format!("{stem}-{suffix}"),
        device,
        kind: Kind::Marks,
        includes: vec![],
        defines: vec![],
        inputs: vec![fixture(&format!("speed-bench/{file}"))],
    }
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    // Kernels run on both cores: every one fits the 16F877A (small
    // flash, tens of RAM bytes), so PIC14 rows pin the software
    // multiply/divide the 14-bit core lowers them to.
    for (stem, file) in [
        ("speed-mul-u8", "speed-mul-u8.c"),
        ("speed-mul-u16", "speed-mul-u16.c"),
        ("speed-mul-u32", "speed-mul-u32.c"),
        ("speed-divmod-u8", "speed-divmod-u8.c"),
        ("speed-divmod-u16", "speed-divmod-u16.c"),
        ("speed-divmod-u32", "speed-divmod-u32.c"),
        ("speed-shift", "speed-shift.c"),
        ("speed-memcpy-32", "speed-memcpy-32.c"),
        ("speed-memset-32", "speed-memset-32.c"),
        ("speed-strlen-strcmp", "speed-strlen-strcmp.c"),
        ("speed-crc16", "speed-crc16.c"),
        ("speed-switch16", "speed-switch16.c"),
        ("speed-struct-scan", "speed-struct-scan.c"),
        ("speed-u16-dec", "speed-u16-dec.c"),
        ("speed-pid-step", "speed-pid-step.c"),
    ] {
        out.push(kernel(stem, "18F4550", file));
        out.push(kernel(stem, "16F877A", file));
    }
    // One PID update through the vendored epic-pid: PIC18 only, like the
    // demo it comes from.
    out.push(Case {
        name: "scen-pid-update-18f4550".to_string(),
        device: "18F4550",
        kind: Kind::Marks,
        includes: [
            "vendor/hal-pic18-base/epic-pid/include",
            "vendor/hal-pic18-base/epic-math/include",
        ]
        .iter()
        .map(|d| fixture(d))
        .collect(),
        defines: vec![],
        inputs: [
            "speed-bench/scen-pid-update.c",
            "vendor/hal-pic18-base/epic-pid/src/pid.c",
            "vendor/hal-pic18-base/epic-math/src/pic18/epic_math_mul.c",
            "vendor/hal-pic18-base/epic-math/src/pic18/epic_math_addsub.c",
        ]
        .iter()
        .map(|f| fixture(f))
        .collect(),
    });
    // One control-loop pass through the vendored control demo
    // (epic-cc#873): PIC18 only, like the demo it comes from. Init only
    // programs Timer2 and never enables GIE, so the pass runs ISR-free
    // with no timer model: the ladder times it like any other Marks
    // row. The vendor oversample and average helpers are __EPIC_CC__
    // stubs, so the pass pins the PID update and PWM duty.
    out.push(Case {
        name: "scen-control-pass-18f4550".to_string(),
        device: "18F4550",
        kind: Kind::Marks,
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
            "speed-bench/scen-control-pass.c",
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
            "vendor/hal-pic18-control-demo/config_18F4550.c",
        ]
        .iter()
        .map(|f| fixture(f))
        .collect(),
    });
    // Interrupt cost, one fixture per core.
    for (name, device, file) in [
        ("speed-isr-18f4550", "18F4550", "speed-isr-18f4550.c"),
        ("speed-isr-16f877a", "16F877A", "speed-isr-16f877a.c"),
    ] {
        out.push(Case {
            name: name.to_string(),
            device,
            kind: Kind::Isr,
            includes: vec![],
            defines: vec![],
            inputs: vec![fixture(&format!("speed-bench/{file}"))],
        });
    }
    out
}

trait MarkSim {
    fn step_one(&mut self);
    fn ram_byte(&self, addr: usize) -> u8;
}

impl MarkSim for pic14_sim::Pic18 {
    fn step_one(&mut self) {
        self.step();
    }
    fn ram_byte(&self, addr: usize) -> u8 {
        self.ram()[addr]
    }
}

impl MarkSim for pic14_sim::Pic14 {
    fn step_one(&mut self) {
        self.step();
    }
    fn ram_byte(&self, addr: usize) -> u8 {
        self.ram()[addr]
    }
}
/// Compile `c` under `profile` (`"Os"` is the default flags, anything
/// else is passed as `-O<profile>`) and return the HEX, the `--map`
/// text, and the size-report stderr. A failing build fails the test:
/// every profile must build every ladder row.
fn compile(c: &Case, profile: &str) -> (String, String, String) {
    let tag = format!("cycle-{}-{profile}-{}", c.name, std::process::id());
    let dir = std::env::temp_dir();
    let hex_path = dir.join(format!("{tag}.hex"));
    let map_path = dir.join(format!("{tag}.map"));
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
    cmd.arg("-o").arg(&hex_path).arg("--map").arg(&map_path);
    cmd.args(&c.inputs);
    let out = cmd.output().expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc {} ({} -O{}): {}",
        c.name,
        c.device,
        profile,
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).expect("read hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let report = String::from_utf8_lossy(&out.stderr).into_owned();
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    (hex, map, report)
}

/// `global <name> 0xNN` address off the compiler's own `--map` output.
/// Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
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

/// Step the sim from the `bench_mark` 1 store to the 2 store and return
/// the cycle delta. The program must halt afterwards: kernels are
/// straight-line, so a non-halting run is a broken fixture, not a slow one.
fn measure_marks(hex: &str, map: &str, device: &str, name: &str) -> u64 {
    let addr = map_addr(map, "bench_mark");
    if device == "18F4550" {
        let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(hex));
        step_to_mark(&mut p, addr, 1, name);
        let c1 = p.cycles();
        step_to_mark(&mut p, addr, 2, name);
        let c2 = p.cycles();
        p.run(5_000_000);
        assert!(p.halted(), "{name}: kernel must halt");
        c2 - c1
    } else {
        let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(hex));
        step_to_mark(&mut p, addr, 1, name);
        let c1 = p.cycles();
        step_to_mark(&mut p, addr, 2, name);
        let c2 = p.cycles();
        p.run(5_000_000);
        assert!(p.halted(), "{name}: kernel must halt");
        c2 - c1
    }
}

fn step_to_mark(p: &mut impl MarkSim, addr: usize, value: u8, name: &str) {
    let mut steps = 0usize;
    while p.ram_byte(addr) != value {
        p.step_one();
        steps += 1;
        assert!(steps < 1_000_000, "{name}: never stored mark {value}");
    }
}

/// Interrupt cost for one ISR fixture: `(latency, round_trip)`. Latency
/// is vector entry to the ISR's first user store across
/// `fire_interrupt`; the round trip is the halt-cycle delta of an
/// interrupted run against a clean baseline run of the same binary (the
/// ISR resumes at the same pc, so the spin count is identical and only
/// vector, save, body, restore and RETFIE remain).
fn measure_isr(hex: &str, map: &str, device: &str, name: &str) -> (u64, u64) {
    let (sync, isr) = (map_addr(map, "bench_sync"), map_addr(map, "bench_isr"));
    if device == "18F4550" {
        let mut base = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(hex));
        base.run(5_000_000);
        assert!(base.halted(), "{name}: baseline must halt");
        let mut q = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(hex));
        arm(&mut q, sync, name);
        for _ in 0..10 {
            q.step();
        }
        let c0 = q.cycles();
        q.fire_interrupt();
        step_to_mark(&mut q, isr, 1, name);
        let latency = q.cycles() - c0;
        q.run(5_000_000);
        assert!(q.halted(), "{name}: interrupted run must halt");
        (latency, q.cycles() - base.cycles())
    } else {
        let mut base = pic14_sim::Pic14::new(pic14_sim::parse_hex(hex));
        base.run(5_000_000);
        assert!(base.halted(), "{name}: baseline must halt");
        let mut q = pic14_sim::Pic14::new(pic14_sim::parse_hex(hex));
        arm(&mut q, sync, name);
        for _ in 0..10 {
            q.step();
        }
        let c0 = q.cycles();
        q.fire_interrupt();
        step_to_mark(&mut q, isr, 1, name);
        let latency = q.cycles() - c0;
        q.run(5_000_000);
        assert!(q.halted(), "{name}: interrupted run must halt");
        (latency, q.cycles() - base.cycles())
    }
}

/// Step to the armed window (`bench_sync == 1`): the spin the fixture
/// sits in while the harness injects the interrupt. Syncing on a store
/// instead of a hardcoded pc keeps the injection point stable across
/// codegen shifts.
fn arm(p: &mut impl MarkSim, sync: usize, name: &str) {
    let mut steps = 0usize;
    while p.ram_byte(sync) != 1 {
        p.step_one();
        steps += 1;
        assert!(steps < 1_000_000, "{name}: fixture never armed");
    }
}

/// A baseline row that records more than the tree produces, so the ladder
/// would absorb a regression of up to `headroom` before noticing. Reported
/// (not fatal) unless the run is strict.
fn drift_report(name: &str, metric: &str, baseline: u64, measured: u64) -> Option<String> {
    if baseline <= measured {
        return None;
    }
    let headroom = baseline - measured;
    Some(format!(
        "{name}: {metric} baseline {baseline} is {headroom} above measured {measured}; \
         a regression of up to {headroom} would go undetected. Re-baseline with \
         UPDATE_CYCLE_BASELINE=1 and diff the result."
    ))
}

/// Machine-readable dump of this run's measurements for tooling. Every
/// field is path-safe, so no escaping is needed, and this avoids a
/// serde_json dev-dependency for one aid.
fn print_report_json(measured: &[BaselineEntry]) {
    println!("SPEED_REPORT_JSON_BEGIN");
    println!("[");
    for (i, e) in measured.iter().enumerate() {
        println!(
            "  {{\"name\": \"{}\", \"device\": \"{}\", \"profile\": \"{}\", \"cycles\": {}, \"flash_words\": {}, \"ram_bytes\": {}}}{}",
            e.name,
            e.device,
            e.profile,
            e.cycles,
            e.flash_words,
            e.ram_bytes,
            if i + 1 == measured.len() { "" } else { "," }
        );
    }
    println!("]");
    println!("SPEED_REPORT_JSON_END");
}

/// Gate one measured triple against its baseline entry: growth fails,
/// and under `strict` so does headroom. Skipped entirely on update runs
/// (which record instead of asserting).
fn check_gated(
    key: &str,
    cycles: u64,
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
            "{key}: no baseline entry (run with UPDATE_CYCLE_BASELINE=1 to add one)"
        ));
        return;
    };
    if cycles > base.cycles {
        failures.push(format!(
            "{key}: cycles grew {} -> {} (+{})",
            base.cycles,
            cycles,
            cycles - base.cycles
        ));
    }
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
        if let Some(msg) = drift_report(key, "cycles", base.cycles, cycles) {
            failures.push(msg);
        }
        if let Some(msg) = drift_report(key, "flash", base.flash_words as u64, flash_words as u64) {
            failures.push(msg);
        }
        if let Some(msg) = drift_report(key, "RAM", base.ram_bytes as u64, ram_bytes as u64) {
            failures.push(msg);
        }
    }
}

#[test]
fn cycle_counts_do_not_regress() {
    let update = std::env::var("UPDATE_CYCLE_BASELINE").is_ok();
    // Strict mode turns a stale baseline row into a failure. CI sets it so
    // headroom cannot accumulate unnoticed; the default stays permissive so
    // a perf change may speed a kernel up without re-baselining mid-iteration.
    let strict = std::env::var("STRICT_CYCLE_BASELINE").is_ok();
    let baseline = load_baseline();
    let mut measured = Vec::new();
    let mut rows = Vec::new();
    let mut failures = Vec::new();

    let cases = cases();
    // Validate the filter before measuring: a typo must fail fast, not
    // after minutes of compiling every case. `-O2` rows filter as
    // `name@O2`, `-Os` rows as the bare name.
    let mut valid = Vec::new();
    for c in &cases {
        // ISR cases measure two rows each (`-latency`, `-roundtrip`),
        // so only the suffixed keys are valid targets: the bare case
        // name matches no measured row.
        if c.kind == Kind::Isr {
            for suffix in ["-latency", "-roundtrip"] {
                let n = format!("{}{suffix}", c.name);
                valid.push(n.clone());
                valid.push(entry_key(&n, "O2"));
            }
        } else {
            valid.push(c.name.clone());
            valid.push(entry_key(&c.name, "O2"));
        }
    }
    let filter = update.then(|| parse_only_filter(&valid)).flatten();
    for c in &cases {
        for profile in ["Os", "O2"] {
            let (hex, map, report) = compile(c, profile);
            let flash_words = parse_after(&report, "flash: ");
            let ram_bytes = parse_after(&report, "RAM: ");
            if c.kind == Kind::Isr {
                let (latency, round_trip) = measure_isr(&hex, &map, c.device, &c.name);
                for (suffix, cycles) in [("-latency", latency), ("-roundtrip", round_trip)] {
                    let name = format!("{}{suffix}", c.name);
                    let key = entry_key(&name, profile);
                    let base = baseline
                        .entry
                        .iter()
                        .find(|e| e.name == name && e.profile == profile)
                        .cloned();
                    check_gated(
                        &key,
                        cycles,
                        flash_words,
                        ram_bytes,
                        base.as_ref(),
                        update,
                        strict,
                        &mut failures,
                    );
                    rows.push(Row {
                        name: key.clone(),
                        device: c.device,
                        cycles,
                        cycles_baseline: base.as_ref().map(|b| b.cycles),
                        flash_words,
                    });
                    measured.push(BaselineEntry {
                        name,
                        device: c.device.to_string(),
                        profile: profile.to_string(),
                        cycles,
                        flash_words,
                        ram_bytes,
                    });
                }
            } else {
                let cycles = measure_marks(&hex, &map, c.device, &c.name);
                let key = entry_key(&c.name, profile);
                let base = baseline
                    .entry
                    .iter()
                    .find(|e| e.name == c.name && e.profile == profile)
                    .cloned();
                check_gated(
                    &key,
                    cycles,
                    flash_words,
                    ram_bytes,
                    base.as_ref(),
                    update,
                    strict,
                    &mut failures,
                );
                rows.push(Row {
                    name: key.clone(),
                    device: c.device,
                    cycles,
                    cycles_baseline: base.as_ref().map(|b| b.cycles),
                    flash_words,
                });
                measured.push(BaselineEntry {
                    name: c.name.clone(),
                    device: c.device.to_string(),
                    profile: profile.to_string(),
                    cycles,
                    flash_words,
                    ram_bytes,
                });
            }
        }
    }
    write_step_summary(&rows);
    if std::env::var("SPEED_REPORT_JSON").is_ok() {
        print_report_json(&measured);
        return;
    }

    if update {
        let (to_save, updated, skipped) = merge_baseline(&baseline, &measured, filter.as_ref());
        save_baseline(&Baseline { entry: to_save });
        if updated.is_empty() && skipped.is_empty() {
            println!("cycle baseline: no changes");
        }
        for line in &updated {
            println!("cycle baseline: {line}");
        }
        for line in &skipped {
            println!("cycle baseline: {line}");
        }
        return;
    }

    assert!(
        failures.is_empty(),
        "cycle regression(s):\n{}\n\nIf intentional, re-baseline with:\n  \
         UPDATE_CYCLE_BASELINE=1 cargo test -p driver --test cycle_ladder_e2e",
        failures.join("\n")
    );
}

/// One ladder entry's measured-vs-baseline cycles, for the step-summary
/// table. `cycles_baseline` is `None` for a case with no recorded
/// baseline yet (only possible with `UPDATE_CYCLE_BASELINE=1`, which is
/// about to add one).
struct Row {
    name: String,
    device: &'static str,
    cycles: u64,
    cycles_baseline: Option<u64>,
    flash_words: u32,
}

fn signed_delta(current: u64, baseline: Option<u64>) -> String {
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

/// Append a markdown table of every case's measured cycles against its
/// baseline to `$GITHUB_STEP_SUMMARY`, when set (CI only, a no-op for a
/// local `cargo test`). Runs unconditionally, on a pass or a failure, so
/// a PR shows exactly what moved and by how much instead of a bare
/// crate-level PASS/FAIL.
fn write_step_summary(rows: &[Row]) {
    let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY") else {
        return;
    };
    let mut out = String::from(
        "\n## Cycle ladder (epic-cc#840)\n\n\
         | Fixture | Device | Cycles | delta | Flash words |\n\
         |---|---|---|---|---|\n",
    );
    for r in rows {
        out.push_str(&format!(
            "| {} | {} | {}{} | {} | {} |\n",
            r.name,
            r.device,
            r.cycles,
            r.cycles_baseline
                .map(|b| format!(" / {b}"))
                .unwrap_or_default(),
            signed_delta(r.cycles, r.cycles_baseline),
            r.flash_words,
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
        drift_report("x", "cycles", 100, 100),
        None,
        "an exact match has no headroom to hide a regression in"
    );
    assert_eq!(
        drift_report("x", "cycles", 100, 120),
        None,
        "a baseline below the measured value is a growth, reported elsewhere"
    );
    let msg = drift_report("bench-x", "cycles", 200, 188)
        .expect("12 cycles of headroom must be reported");
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
                cycles: 100,
                flash_words: 10,
                ram_bytes: 4,
            },
            BaselineEntry {
                name: "b".into(),
                device: "d".into(),
                profile: "Os".into(),
                cycles: 200,
                flash_words: 20,
                ram_bytes: 8,
            },
        ],
    };
    let measured = vec![
        BaselineEntry {
            name: "a".into(),
            device: "d".into(),
            profile: "Os".into(),
            cycles: 110,
            flash_words: 10,
            ram_bytes: 4,
        },
        BaselineEntry {
            name: "b".into(),
            device: "d".into(),
            profile: "Os".into(),
            cycles: 190,
            flash_words: 20,
            ram_bytes: 8,
        },
    ];
    let filter: std::collections::HashSet<String> = ["a".to_string()].into_iter().collect();
    let (saved, updated, skipped) = merge_baseline(&baseline, &measured, Some(&filter));
    assert_eq!(
        saved[0].cycles, 110,
        "filtered row takes the measured value"
    );
    assert_eq!(
        saved[1].cycles, 200,
        "foreign speedup is kept at baseline, the gain lands with its owner"
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
fn merge_baseline_keys_o2_rows_apart_from_os_rows() {
    let baseline = Baseline {
        entry: vec![BaselineEntry {
            name: "a".into(),
            device: "d".into(),
            profile: "Os".into(),
            cycles: 100,
            flash_words: 10,
            ram_bytes: 4,
        }],
    };
    let measured = vec![
        BaselineEntry {
            name: "a".into(),
            device: "d".into(),
            profile: "Os".into(),
            cycles: 100,
            flash_words: 10,
            ram_bytes: 4,
        },
        BaselineEntry {
            name: "a".into(),
            device: "d".into(),
            profile: "O2".into(),
            cycles: 90,
            flash_words: 12,
            ram_bytes: 4,
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
