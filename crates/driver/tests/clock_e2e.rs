//! Clock resolution acceptance (epic-cc#691, docs/46 D-4): the sources,
//! their agreement, the header line, and the delay-macro error.

use std::process::Command;

fn driver() -> Command {
    Command::new(env!("CARGO_BIN_EXE_epic-cc"))
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
        + &String::from_utf8_lossy(&out.stdout).to_string()
}

fn write_c(tag: &str, body: &str) -> (String, String) {
    let dir = std::env::temp_dir().join(format!("epic-clock-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let c_path = dir.join("prog.c");
    std::fs::write(&c_path, body).unwrap();
    let hex_path = dir.join("prog.hex");
    (c_path.display().to_string(), hex_path.display().to_string())
}

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

/// The ticket's acceptance program: a crystal-less `#pragma config
/// FOSC = XT` blink at 4 MHz builds, prints the header line, and each
/// 500 ms toggle costs exactly 500000 instruction cycles in the sim.
#[test]
fn tutorial_blink_builds_and_holds_its_cycle_count() {
    let hex_path = std::env::temp_dir().join(format!("clock_blink_{}.hex", std::process::id()));
    let map_path = std::env::temp_dir().join(format!("clock_blink_{}.map", std::process::id()));
    let out = driver()
        .args([
            "tests/fixtures/clock_blink.c",
            "-o",
            hex_path.to_str().unwrap(),
            "--map",
            map_path.to_str().unwrap(),
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    let err = stderr_of(&out);
    assert!(out.status.success(), "driver failed: {err}");
    assert!(err.contains("PIC16F877A @ 4 MHz (XT)"), "{err}");
    let hex = std::fs::read_to_string(&hex_path).expect("read hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let done_addr = map_addr(&map, "done");
    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
    p.run(2_000_000);
    assert!(p.halted(), "blink must halt");
    assert_eq!(p.ram()[done_addr], 0, "done mismatch");
    assert!(
        (1_000_000..1_100_000).contains(&p.cycles()),
        "cycles {} outside the two-toggle window",
        p.cycles()
    );
}

/// Two fixed clocks that disagree fail the build naming both values.
#[test]
fn disagreeing_clocks_fail_naming_both_values() {
    let (c, hex) = write_c(
        "disagree",
        "#include <epic-cc.h>\nEPIC_CONFIG(\"osc=xt, xtal_hz=20000000\");\n#define _XTAL_FREQ 4000000\nvoid main(void) {}\n",
    );
    let out = driver()
        .args([c.as_str(), "-o", hex.as_str(), "--device", "p16f877a"])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&c).parent().unwrap());
    let err = stderr_of(&out);
    assert!(!out.status.success());
    assert!(err.contains("20000000"), "{err}");
    assert!(err.contains("4000000"), "{err}");
}

/// A delay macro with no clock fixed anywhere fails naming the three
/// ways to give one.
#[test]
fn delay_without_any_clock_names_the_three_ways() {
    let (c, hex) = write_c(
        "nodelay",
        "#include <xc.h>\nvoid main(void) { __delay_ms(1); }\n",
    );
    let out = driver()
        .args([c.as_str(), "-o", hex.as_str(), "--device", "p16f877a"])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&c).parent().unwrap());
    let err = stderr_of(&out);
    assert!(!out.status.success());
    assert!(err.contains("_XTAL_FREQ"), "{err}");
    assert!(err.contains("--f-cpu"), "{err}");
    assert!(err.contains("EPIC_CONFIG"), "{err}");
}

/// The board flag fixes the clock (and the header line) with no config
/// and no define in the code.
#[test]
fn f_cpu_alone_fixes_the_clock() {
    let (c, hex) = write_c("fcpu", "void main(void) {}\n");
    let out = driver()
        .args([
            c.as_str(),
            "-o",
            hex.as_str(),
            "--device",
            "p16f877a",
            "--f-cpu",
            "4000000",
        ])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&c).parent().unwrap());
    let err = stderr_of(&out);
    assert!(out.status.success(), "driver failed: {err}");
    assert!(err.contains("PIC16F877A @ 4 MHz"), "{err}");
}

#[test]
fn delay_with_a_known_board_clock_still_needs_the_define() {
    // --f-cpu fixes the build clock but __delay_ms expands _XTAL_FREQ, so
    // the error names the expected value and its source.
    let (c, hex) = write_c(
        "expand",
        "#include <xc.h>\nvoid main(void) { __delay_ms(1); }\n",
    );
    let out = driver()
        .args([
            c.as_str(),
            "-o",
            hex.as_str(),
            "--device",
            "p16f877a",
            "--f-cpu",
            "4000000",
        ])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&c).parent().unwrap());
    let err = stderr_of(&out);
    assert!(!out.status.success());
    assert!(err.contains("expands _XTAL_FREQ"), "{err}");
    assert!(err.contains("4000000"), "{err}");
}

#[test]
fn computed_xtal_freq_builds_without_a_driver_clock() {
    // clang folds the expression for the delays; the driver cannot use it
    // for agreement, so the build clock stays unknown and the build succeeds.
    let (c, hex) = write_c(
        "computed",
        "#include <xc.h>\n#define _XTAL_FREQ (8000000/2)\nvoid main(void) { __delay_ms(1); }\n",
    );
    let out = driver()
        .args([c.as_str(), "-o", hex.as_str(), "--device", "p16f877a"])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&c).parent().unwrap());
    let err = stderr_of(&out);
    assert!(out.status.success(), "driver failed: {err}");
    assert!(err.contains("PIC16F877A @ unknown"), "{err}");
}

#[test]
fn dash_d_xtal_freq_counts_as_a_source() {
    // A -D define feeds clang the macro, so delays build and the clock
    // resolves from it.
    let (c, hex) = write_c(
        "dashd",
        "#include <xc.h>\nvoid main(void) { __delay_ms(1); }\n",
    );
    let out = driver()
        .args([
            c.as_str(),
            "-o",
            hex.as_str(),
            "--device",
            "p16f877a",
            "-D_XTAL_FREQ=4000000",
        ])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&c).parent().unwrap());
    let err = stderr_of(&out);
    assert!(out.status.success(), "driver failed: {err}");
    assert!(err.contains("PIC16F877A @ 4 MHz"), "{err}");
}
