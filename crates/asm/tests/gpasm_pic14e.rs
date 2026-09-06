use asm::assemble_file_to_hex;
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn pic14e_probe_hex_matches_gpasm() {
    let src = include_str!("fixtures/pic14e_probe.asm");
    let ours = assemble_file_to_hex(&device::PIC16F1937, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/pic14e_probe_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args([
            "-p",
            "p16f1937",
            "pic14e_probe.asm",
            "-o",
            "pic14e_probe_gpasm.hex",
        ])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(
        out.status.success(),
        "gpasm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let theirs = std::fs::read_to_string(format!("{dir}/pic14e_probe_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
}
