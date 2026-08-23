use std::path::PathBuf;
use std::process::Command;

fn tmp_hex(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "epic-cc-device-flag-{}-{}-{}.hex",
        name,
        std::process::id(),
        format!("{:?}", std::thread::current().id())
            .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
    ));
    p
}

fn fixture_add() -> String {
    format!("{}/tests/fixtures/add.c", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn device_flag_p18f4550_produces_pic18_hex() {
    let out = tmp_hex("p18f4550");
    let fixture = fixture_add();
    let res = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            fixture.as_str(),
            "-o",
            out.to_str().unwrap(),
            "--device",
            "p18f4550",
        ])
        .output()
        .expect("run driver");
    assert!(
        res.status.success(),
        "driver --device p18f4550 failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let hex = std::fs::read_to_string(&out).expect("read hex");
    let prog = pic14_sim::parse_hex_pic18(&hex);
    assert_eq!(prog.len(), 0x4000, "PIC18 prog should be 0x4000 words");
    let mut sim = pic14_sim::Pic18::new(prog);
    sim.run(10_000);
    assert!(sim.halted(), "Pic18 sim should halt (SLEEP) for add.c");
    let _ = std::fs::remove_file(&out);
}

#[test]
fn device_default_is_pic16() {
    let out = tmp_hex("default-p16");
    let fixture = fixture_add();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args([fixture.as_str(), "-o", out.to_str().unwrap()]);
    cmd.env_remove("PIC8_DEVICE");
    let res = cmd.output().expect("run driver");
    assert!(
        res.status.success(),
        "driver default (no --device) failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let hex = std::fs::read_to_string(&out).expect("read hex");
    let prog = pic14_sim::parse_hex(&hex);
    assert_eq!(prog.len(), 8192, "PIC16 prog should be 8192 words");
    let mut sim = pic14_sim::Pic14::new(prog);
    sim.run(10_000);
    assert!(
        sim.halted(),
        "Pic14 sim should halt for add.c with default device"
    );
    let _ = std::fs::remove_file(&out);
}

#[test]
fn device_flag_case_insensitive() {
    let out = tmp_hex("case-insensitive");
    let fixture = fixture_add();
    let res = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            fixture.as_str(),
            "-o",
            out.to_str().unwrap(),
            "--device",
            "P18F4550",
        ])
        .output()
        .expect("run driver");
    assert!(
        res.status.success(),
        "driver --device P18F4550 (uppercase) failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let hex = std::fs::read_to_string(&out).unwrap();
    let prog = pic14_sim::parse_hex_pic18(&hex);
    assert_eq!(prog.len(), 0x4000);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn device_env_fallback_pic18() {
    let out = tmp_hex("env-p18");
    let fixture = fixture_add();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args([fixture.as_str(), "-o", out.to_str().unwrap()]);
    cmd.env("PIC8_DEVICE", "p18f4550");
    let res = cmd.output().expect("run driver");
    assert!(
        res.status.success(),
        "driver PIC8_DEVICE=p18f4550 failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let hex = std::fs::read_to_string(&out).unwrap();
    let prog = pic14_sim::parse_hex_pic18(&hex);
    assert_eq!(prog.len(), 0x4000);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn device_env_default_fallback_is_pic16() {
    let out = tmp_hex("env-default");
    let fixture = fixture_add();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args([fixture.as_str(), "-o", out.to_str().unwrap()]);
    cmd.env_remove("PIC8_DEVICE");
    let res = cmd.output().expect("run driver");
    assert!(
        res.status.success(),
        "default env fallback failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let hex = std::fs::read_to_string(&out).unwrap();
    let _prog = pic14_sim::parse_hex(&hex);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn device_unknown_exits_1() {
    let out = tmp_hex("unknown");
    let fixture = fixture_add();
    let res = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            fixture.as_str(),
            "-o",
            out.to_str().unwrap(),
            "--device",
            "p99f9999",
        ])
        .output()
        .expect("run driver");
    assert!(!res.status.success(), "unknown device should fail");
    assert_eq!(
        res.status.code(),
        Some(1),
        "unknown device must exit 1, got {:?} stderr: {}",
        res.status.code(),
        String::from_utf8_lossy(&res.stderr)
    );
    let stderr = String::from_utf8_lossy(&res.stderr).to_ascii_lowercase();
    assert!(
        stderr.contains("unknown device"),
        "stderr should mention unknown device: {stderr}"
    );
    let _ = std::fs::remove_file(&out);
}

#[test]
fn device_flag_overrides_env() {
    let out = tmp_hex("override");
    let fixture = fixture_add();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args([
        fixture.as_str(),
        "-o",
        out.to_str().unwrap(),
        "--device",
        "p16f877a",
    ]);
    cmd.env("PIC8_DEVICE", "p18f4550");
    let res = cmd.output().expect("run driver");
    assert!(
        res.status.success(),
        "flag should override env: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    let hex = std::fs::read_to_string(&out).unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    assert_eq!(
        prog.len(),
        8192,
        "flag p16f877a should produce PIC14 hex even when env is p18"
    );
    let _ = std::fs::remove_file(&out);
}

#[test]
fn device_flag_accepts_the_hal_manifest_spelling() {
    // epic-hal's manifest names variants `16F877A`, and XC8 takes `16f877a`.
    // Both must reach the compiler without a caller-side mapping table.
    for spelling in ["16F877A", "16f877a", "PIC16F877A"] {
        let out = tmp_hex(&format!("hal-spelling-{spelling}"));
        let fixture = fixture_add();
        let res = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
            .args([
                fixture.as_str(),
                "-o",
                out.to_str().unwrap(),
                "--target",
                spelling,
            ])
            .output()
            .expect("run driver");
        assert!(
            res.status.success(),
            "--target {spelling} failed: {}",
            String::from_utf8_lossy(&res.stderr)
        );
        let hex = std::fs::read_to_string(&out).expect("read hex");
        assert_eq!(pic14_sim::parse_hex(&hex).len(), 8192);
        let _ = std::fs::remove_file(&out);
    }
}
