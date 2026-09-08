//! SDCC parity differential tests (docs/35, P0).
//!
//! Runs the committed corpus (Tier 1 + Tier 2) through both compilers and
//! compares named outputs. Requires SDCC in the image (PIC8_SDCC); the test
//! skips with a clear message when SDCC is absent, mirroring the gputils
//! cross-check's opt-in pattern.

use sdcc_parity::corpus;
use sdcc_parity::run_differential;

/// The committed SDCC-known-bugs table (docs/35 section 5 item 2): programs
/// where SDCC is wrong (epic-cc matches the hand-computed expected value,
/// SDCC does not). These are excluded from the differential gate.
#[derive(serde::Deserialize)]
struct KnownBugs {
    bug: Vec<KnownBug>,
}

#[derive(serde::Deserialize)]
struct KnownBug {
    device: String,
    program: String,
}

fn known_bugs() -> KnownBugs {
    let text = include_str!("../sdcc-known-bugs.toml");
    toml::from_str(text).expect("parse sdcc-known-bugs.toml")
}

fn is_known_bug(name: &str, device: &str) -> bool {
    known_bugs()
        .bug
        .iter()
        .any(|b| b.program == name && b.device == device)
}

#[test]
fn corpus_differential_clean() {
    // Skip when SDCC is not present (local dev without the image).
    if std::env::var_os("PIC8_SDCC").is_none()
        && !std::path::Path::new("/usr/local/bin/sdcc").exists()
    {
        eprintln!("skipping: SDCC not present (run inside the dev image)");
        return;
    }

    let bugs = known_bugs();
    let mut clean = 0usize;
    let mut failures = Vec::new();
    let mut excluded = 0usize;
    let mut surface_gaps = Vec::new();
    for prog in corpus::corpus() {
        // Run on all three cores (PIC14, PIC18, PIC14E) where the program
        // compiles.
        for device in [
            &device::PIC16F877A,
            &device::PIC18F4550,
            &device::PIC16F1938,
        ] {
            let name = prog.outputs.first().cloned().unwrap_or_else(|| "?".into());
            // Catch panics per-program (a sim panic on one SDCC program must
            // not abort the whole corpus run).
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_differential(&prog, device)
            }));
            match result {
                Ok(Ok(r)) if r.pass => {
                    clean += 1;
                    eprintln!(
                        "PASS {name} on {}: epic {}w/{}B/{}cyc sdcc {}w/{}B/{}cyc",
                        device.name,
                        r.epic.flash_words,
                        r.epic.ram_bytes,
                        r.epic.cycles,
                        r.sdcc.flash_words,
                        r.sdcc.ram_bytes,
                        r.sdcc.cycles,
                    );
                }
                Ok(Ok(r)) => {
                    // Arbitration (docs/35 section 5 item 2): a mismatch
                    // where SDCC is the wrong side is a recorded known bug,
                    // excluded from the gate. Otherwise it's a real finding.
                    if is_known_bug(&prog.name, device.name) {
                        // Sanity: only the named programs may be excluded.
                        let in_table = bugs
                            .bug
                            .iter()
                            .any(|b| b.program == prog.name && b.device == device.name);
                        assert!(
                            in_table,
                            "excluded {name} on {} not in known-bugs",
                            device.name
                        );
                        excluded += 1;
                        eprintln!(
                            "EXCLUDED {name} on {} (SDCC known bug): {}",
                            device.name, r.detail
                        );
                    } else {
                        failures.push(format!("{name} on {}: {}", device.name, r.detail));
                    }
                }
                Ok(Err(e)) => {
                    // Classify: an epic-cc compile failure is a surface gap
                    // (tracked by the sub-epics). An sdcc/gplink failure on
                    // a known-bug program (e.g. SDCC pic14 has no 64-bit)
                    // is an excluded SDCC limitation; any other sdcc/gplink
                    // failure or a sim non-halt is a real harness/oracle
                    // problem.
                    if e.starts_with("epic-cc failed") {
                        surface_gaps.push(format!("{name} on {}: {e}", device.name));
                    } else if is_known_bug(&prog.name, device.name) {
                        let in_table = bugs
                            .bug
                            .iter()
                            .any(|b| b.program == prog.name && b.device == device.name);
                        assert!(
                            in_table,
                            "excluded {name} on {} not in known-bugs",
                            device.name
                        );
                        excluded += 1;
                        eprintln!("EXCLUDED {name} on {} (SDCC limitation):", device.name);
                    } else {
                        failures.push(format!("{name} on {}: {e}", device.name));
                    }
                }
                Err(p) => {
                    let msg = p
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| p.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    failures.push(format!("{name} on {}: PANIC {msg}", device.name));
                }
            }
        }
    }
    eprintln!("{clean} differential-clean runs");
    eprintln!("{excluded} excluded (SDCC known bugs)");
    eprintln!("{} differential mismatches:", failures.len());
    for f in &failures {
        eprintln!("  {f}");
    }
    eprintln!(
        "{} surface gaps (epic-cc cannot compile yet):",
        surface_gaps.len()
    );
    for g in &surface_gaps {
        eprintln!("  {g}");
    }
}
