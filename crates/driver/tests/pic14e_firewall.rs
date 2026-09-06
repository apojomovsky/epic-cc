//! The `pic14e` firewall at the driver level: the core has no backend yet
//! (P1), so the driver must refuse it with the named message rather than
//! emit anything. The device TOMLs land in P0 (D-3), so this is the check
//! that the data is live while the codegen stays off.

use std::process::Command;

#[test]
fn driver_refuses_pic14e_with_the_firewall_message() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/add.c",
            "-o",
            "/tmp/pic14e_firewall.hex",
            "--target",
            "p16f1937",
        ])
        .output()
        .expect("run driver");
    assert!(!out.status.success(), "pic14e compile must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no backend yet"),
        "firewall message missing: {stderr}"
    );
    assert!(
        stderr.contains("p16f1937"),
        "firewall must name the device: {stderr}"
    );
}
