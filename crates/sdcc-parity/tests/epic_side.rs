//! Epic-cc side of the SDCC parity harness, tested in isolation (SDCC is
//! not required for this test; it exercises compile + map parse + sim).

use sdcc_parity::corpus;
use sdcc_parity::run_epic;

#[test]
fn epic_side_compiles_and_runs_corpus() {
    // The epic-cc driver needs the pinned clang; skip if absent (local dev
    // without the image).
    if std::env::var_os("PIC8_CLANG_UNWRAPPED").is_none()
        && !std::path::Path::new("/opt/clang/bin/clang").exists()
    {
        eprintln!("skipping: clang not present (run inside the dev image)");
        return;
    }

    let mut clean = 0usize;
    let mut failures = Vec::new();
    let mut conformance: Vec<String> = Vec::new();
    for prog in corpus::corpus() {
        for device in [&device::PIC16F877A, &device::PIC18F4550] {
            if !prog.runs_on(device.name) {
                continue;
            }
            match run_epic(&prog, device) {
                Ok(r) => {
                    // Conformance (docs/35 section 5 item 1): the run must
                    // land on the hand-computed value, independent of what
                    // SDCC does.
                    let out = prog.outputs.first();
                    let got = out.and_then(|o| r.outputs.get(o)).copied();
                    if got != Some(prog.expected as u32) {
                        conformance.push(format!(
                            "{} on {}: {:?} != expected {:#04x}",
                            prog.name, device.name, got, prog.expected
                        ));
                        continue;
                    }
                    clean += 1;
                    eprintln!(
                        "epic {} on {}: {}w/{}B/{}cyc",
                        prog.outputs.first().unwrap_or(&"?".into()),
                        device.name,
                        r.flash_words,
                        r.ram_bytes,
                        r.cycles,
                    );
                }
                Err(e) => {
                    // Known epic-cc gaps (surface probes) are expected to
                    // fail to compile; the differential test reports them,
                    // the epic-side test only asserts the harness runs.
                    eprintln!(
                        "epic {} on {}: {e} (known gap or harness issue)",
                        prog.outputs.first().unwrap_or(&"?".into()),
                        device.name
                    );
                    failures.push(format!(
                        "{} on {}: {e}",
                        prog.outputs.first().unwrap_or(&"?".into()),
                        device.name
                    ));
                }
            }
        }
    }
    // Conformance (docs/35 section 5 item 1) is a hard gate here; the
    // other failures are surface gaps (a capability epic-cc cannot
    // compile yet, tracked by a sub-epic) and harness crashes.
    let harness_failures: Vec<_> = failures
        .iter()
        .filter(|f| f.contains("no global"))
        .cloned()
        .collect();
    assert!(
        conformance.is_empty(),
        "{} conformance failures:\n{}",
        conformance.len(),
        conformance.join("\n")
    );
    assert!(
        harness_failures.is_empty(),
        "{} harness failures:\n{}",
        harness_failures.len(),
        harness_failures.join("\n")
    );
    eprintln!(
        "{clean} epic-side runs; {} known-gap compile failures",
        failures.len()
    );
}
