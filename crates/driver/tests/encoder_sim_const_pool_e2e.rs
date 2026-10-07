//! epic-cc#931 (with epic-cc#923): the `epic-encoder` sim slice links
//! under `--const-pool`. `main` spans pages as a single planned chunk while
//! the final banking grows it across a page boundary; isel now measures the
//! real final-context sizes and repairs (re-split, re-place) instead of
//! shipping the straddle. The fixture is the vendored
//! `hal-pic16-encoder-full` tree plus the four sim-variant files from the
//! same epic-hal pin (see its PROVENANCE.md), compiled like
//! `epic_build.py --module epic-encoder --variant sim` with `--const-pool`.

use std::process::Command;

fn sim_case() -> (Vec<String>, Vec<String>, Vec<String>) {
    let root = "tests/fixtures/vendor/hal-pic16-encoder-full";
    let includes = [
        "hal/pic14/16f87xa/include/epiccc",
        "hal/pic14/16f87xa/include",
        "hal/pic14/core/include",
        "common/include",
        "lib/tick/include",
        "lib/encoder/include",
        "lib/serial/include",
    ]
    .iter()
    .map(|d| format!("{root}/{d}"))
    .collect();
    let inputs = [
        "hal/pic14/core/src/peripherals/pic14_timer2.c",
        "hal/pic14/core/src/peripherals/pic14_usart.c",
        "hal/pic14/core/src/core/pic14_irq.c",
        "hal/pic14/16f87xa/src/core/pic16_irq_table.c",
        "hal/pic14/core/src/epiccc/pic16_isr_vector.c",
        "hal/pic14/core/src/epiccc/pic16_irq_dispatch_serial_tick_epiccc.c",
        "hal/pic14/16f87xa/src/mdb/pic16_harness_mdb.c",
        "lib/tick/src/epic_tick.c",
        "lib/encoder/src/encoder.c",
        "lib/encoder/tests/sim_encoder.c",
        "config.c",
    ]
    .iter()
    .map(|f| format!("{root}/{f}"))
    .collect();
    let defines = ["PIC16F877A", "FOSC_HZ=20000000", "__EPIC_CC__"]
        .iter()
        .map(|d| d.to_string())
        .collect();
    (includes, inputs, defines)
}

fn build_sim() -> Vec<u16> {
    let (includes, inputs, defines) = sim_case();
    let hex_path =
        std::env::temp_dir().join(format!("encoder_sim_pool_{}.hex", std::process::id()));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args(["--target", "16F877A", "--const-pool"]);
    for dir in &includes {
        cmd.args(["-I", dir]);
    }
    for def in &defines {
        cmd.args(["-D", def]);
    }
    cmd.arg("-o").arg(&hex_path).args(&inputs);
    let out = cmd.output().expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc failed on the encoder-sim slice: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read produced hex");
    let _ = std::fs::remove_file(&hex_path);
    pic14_sim::parse_hex(&produced)
}

#[test]
fn encoder_sim_links_under_const_pool() {
    // Failing-before: isel's post-banking page-fit panic, `main` spanning
    // pages 0x1000-0x1805 as a single extent while cross-page chunking did
    // not apply. Passing-after: the repaired layout links (the driver gates
    // flash fit in its own report, so a successful build fits the device).
    let words = build_sim();
    assert!(!words.is_empty(), "the sim build must produce flash words");
}
