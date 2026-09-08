//! The SDCC parity regression gate (docs/35, P0): "a ratio regression
//! fails CI".
//!
//! Runs the committed corpus (Tier 1 + Tier 2) through both compilers and
//! compares every row against the committed `baseline.toml`, the same
//! fail-on-regression mechanism as the driver's size_regression suite:
//!
//! - a differential-clean row stays clean, and its epic-cc/SDCC flash
//!   and cycle ratios must not regress (improving is free);
//! - a known-bug row must keep epic-cc on the hand-computed expected
//!   value; if epic-cc also misses, that is an epic-cc bug and the gate
//!   fails regardless of what SDCC did;
//! - a row whose SDCC side changed at all (it started matching, or a
//!   compile error appeared) fails: re-arbitrate and re-baseline
//!   deliberately.
//!
//! `UPDATE_SDCC_BASELINE=1` rewrites `baseline.toml` from the current
//! run. Requires SDCC in the image; skips with a clear message otherwise.

use sdcc_parity::corpus;
use sdcc_parity::{run_differential, run_epic};

#[derive(serde::Deserialize, serde::Serialize, PartialEq, Eq, Clone, Copy, Debug)]
#[serde(rename_all = "kebab-case")]
enum Status {
    /// Differential-clean: both accepted, outputs agree.
    Pass,
    /// Arbitrated SDCC bug/limitation, keyed in sdcc-known-bugs.toml.
    KnownBug,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
struct Row {
    program: String,
    device: String,
    status: Status,
    epic_flash: usize,
    sdcc_flash: usize,
    epic_cycles: usize,
    sdcc_cycles: usize,
}

fn baseline_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("baseline.toml")
}

fn load_baseline() -> Vec<Row> {
    let text = std::fs::read_to_string(baseline_path()).expect("read baseline.toml");
    toml::from_str::<BaselineToml>(&text)
        .expect("parse baseline.toml")
        .row
}

#[derive(serde::Deserialize, serde::Serialize)]
struct BaselineToml {
    row: Vec<Row>,
}

fn save_baseline(rows: &[Row]) {
    let header = "# SDCC parity ratio baseline (docs/35, P0). Do not hand-edit the\n\
                  # numbers; regenerate inside the dev image with:\n\
                  #   UPDATE_SDCC_BASELINE=1 cargo test -p sdcc-parity --test regression_gate\n\
                  # then diff and commit deliberately. epic-cc/SDCC flash and\n\
                  # cycle ratios must not regress against these rows; improving\n\
                  # is free.\n";
    let toml_body =
        toml::to_string_pretty(&BaselineToml { row: rows.to_vec() }).expect("serialize baseline");
    std::fs::write(baseline_path(), format!("{header}{toml_body}")).expect("write baseline.toml");
}

#[derive(serde::Deserialize, Clone)]
struct KnownBug {
    device: String,
    program: String,
    /// The hand-computed expected value epic-cc must keep producing.
    expected: i64,
}

#[derive(serde::Deserialize)]
struct KnownBugs {
    bug: Vec<KnownBug>,
}

fn known_bugs() -> KnownBugs {
    let text = include_str!("../sdcc-known-bugs.toml");
    toml::from_str(text).expect("parse sdcc-known-bugs.toml")
}

fn known_bug_for(name: &str, device: &str) -> Option<KnownBug> {
    known_bugs()
        .bug
        .iter()
        .find(|b| b.program == name && b.device == device)
        .cloned()
}

fn find<'a>(rows: &'a [Row], program: &str, device: &str) -> Option<&'a Row> {
    rows.iter()
        .find(|r| r.program == program && r.device == device)
}

/// True when the new epic-cc/SDCC ratio regresses against the baseline's:
/// `new_epic/new_sdcc > base_epic/base_sdcc`, cross-multiplied in integer
/// arithmetic so it is exact. Improving is free.
fn ratio_regressed(base_epic: usize, base_sdcc: usize, new_epic: usize, new_sdcc: usize) -> bool {
    if base_sdcc == 0 || new_sdcc == 0 {
        return false; // no comparable denominator; nothing to gate
    }
    new_epic * base_sdcc > base_epic * new_sdcc
}

#[test]
fn corpus_matches_ratio_baseline() {
    if std::env::var_os("PIC8_SDCC").is_none()
        && !std::path::Path::new("/usr/local/bin/sdcc").exists()
    {
        eprintln!("skipping: SDCC not present (run inside the dev image)");
        return;
    }

    let update = std::env::var_os("UPDATE_SDCC_BASELINE").is_some();
    let mut measured: Vec<Row> = Vec::new();
    let mut problems: Vec<String> = Vec::new();

    for prog in corpus::corpus() {
        for device in [
            &device::PIC16F877A,
            &device::PIC18F4550,
            &device::PIC16F1938,
        ] {
            let key = format!("{} on {}", prog.name, device.name);
            let bug = known_bug_for(&prog.name, device.name);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_differential(&prog, device)
            }));
            match result {
                Err(p) => {
                    let msg = p
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| p.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    problems.push(format!("{key}: PANIC {msg}"));
                }
                Ok(Err(e)) => {
                    // An error is only acceptable on an arbitrated
                    // limitation row (e.g. SDCC error 206 on i64), and the
                    // epic-cc side must still hit the hand-computed value.
                    match bug {
                        Some(bug) => {
                            if e.starts_with("epic-cc failed") {
                                problems.push(format!("{key}: epic-cc failed: {e}"));
                                continue;
                            }
                            match run_epic(&prog, device) {
                                Ok(epic) => {
                                    let out = prog.outputs.first();
                                    let got = out.and_then(|o| epic.outputs.get(o)).copied();
                                    if got != Some(bug.expected as u32) {
                                        problems.push(format!(
                                            "{key}: epic-cc drifted off the expected value \
                                             ({got:?} != {}): {e}",
                                            bug.expected
                                        ));
                                        continue;
                                    }
                                    measured.push(Row {
                                        program: prog.name.clone(),
                                        device: device.name.to_string(),
                                        status: Status::KnownBug,
                                        epic_flash: epic.flash_words,
                                        sdcc_flash: 0,
                                        epic_cycles: epic.cycles,
                                        sdcc_cycles: 0,
                                    });
                                }
                                Err(e) => problems.push(format!("{key}: epic-cc failed: {e}")),
                            }
                        }
                        None => problems.push(format!("{key}: run failed: {e}")),
                    }
                }
                Ok(Ok(r)) => {
                    if r.pass {
                        measured.push(Row {
                            program: prog.name.clone(),
                            device: device.name.to_string(),
                            status: Status::Pass,
                            epic_flash: r.epic.flash_words,
                            sdcc_flash: r.sdcc.flash_words,
                            epic_cycles: r.epic.cycles,
                            sdcc_cycles: r.sdcc.cycles,
                        });
                    } else if let Some(bug) = bug {
                        // Arbitrated row: SDCC still wrong, but epic-cc must
                        // still be right (that is what makes it an SDCC bug).
                        let out = prog.outputs.first();
                        let got = out.and_then(|o| r.epic.outputs.get(o)).copied();
                        if got != Some(bug.expected as u32) {
                            problems.push(format!(
                                "{key}: epic-cc drifted off the expected value ({got:?} != {}): {}",
                                bug.expected, r.detail
                            ));
                            continue;
                        }
                        measured.push(Row {
                            program: prog.name.clone(),
                            device: device.name.to_string(),
                            status: Status::KnownBug,
                            epic_flash: r.epic.flash_words,
                            sdcc_flash: r.sdcc.flash_words,
                            epic_cycles: r.epic.cycles,
                            sdcc_cycles: r.sdcc.cycles,
                        });
                    } else {
                        problems.push(format!(
                            "{key}: new differential mismatch (not arbitrated): {}",
                            r.detail
                        ));
                    }
                }
            }
        }
    }

    if update {
        save_baseline(&measured);
        eprintln!("baseline.toml rewritten with {} rows", measured.len());
        return;
    }

    let baseline = load_baseline();

    for row in &measured {
        let Some(base) = find(&baseline, &row.program, &row.device) else {
            problems.push(format!(
                "{} on {}: no baseline row (add it deliberately or re-baseline)",
                row.program, row.device
            ));
            continue;
        };
        match (base.status, row.status) {
            (Status::Pass, Status::Pass) => {
                if ratio_regressed(
                    base.epic_flash,
                    base.sdcc_flash,
                    row.epic_flash,
                    row.sdcc_flash,
                ) {
                    problems.push(format!(
                        "{} on {}: flash ratio regressed ({}w/{}w from {}w/{}w)",
                        row.program,
                        row.device,
                        row.epic_flash,
                        row.sdcc_flash,
                        base.epic_flash,
                        base.sdcc_flash
                    ));
                }
                if ratio_regressed(
                    base.epic_cycles,
                    base.sdcc_cycles,
                    row.epic_cycles,
                    row.sdcc_cycles,
                ) {
                    problems.push(format!(
                        "{} on {}: cycle ratio regressed ({}c/{}c from {}c/{}c)",
                        row.program,
                        row.device,
                        row.epic_cycles,
                        row.sdcc_cycles,
                        base.epic_cycles,
                        base.sdcc_cycles
                    ));
                }
            }
            (Status::KnownBug, Status::KnownBug) => {
                // The SDCC side's measurability is part of the
                // arbitration: a limitation row is SDCC-rejects-the-
                // program (no numbers), a bug row is SDCC-links-and-
                // runs. A row that switches between the two means the
                // oracle's behavior changed and the entry is stale.
                if (base.sdcc_flash > 0) != (row.sdcc_flash > 0) {
                    problems.push(format!(
                        "{} on {}: SDCC side changed ({} -> {}); re-arbitrate and re-baseline",
                        row.program,
                        row.device,
                        if base.sdcc_flash > 0 {
                            "linked"
                        } else {
                            "rejected"
                        },
                        if row.sdcc_flash > 0 {
                            "linked"
                        } else {
                            "rejected"
                        },
                    ));
                }
            }
            (Status::Pass, Status::KnownBug) => problems.push(format!(
                "{} on {}: was differential-clean, now mismatches",
                row.program, row.device
            )),
            (Status::KnownBug, Status::Pass) => problems.push(format!(
                "{} on {}: SDCC behavior changed (now matches); re-arbitrate and re-baseline",
                row.program, row.device
            )),
        }
    }
    for base in &baseline {
        if find(&measured, &base.program, &base.device).is_none() {
            problems.push(format!(
                "{} on {}: baseline row missing from this run",
                base.program, base.device
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "SDCC parity regressions:\n  {}",
        problems.join("\n  ")
    );
}
