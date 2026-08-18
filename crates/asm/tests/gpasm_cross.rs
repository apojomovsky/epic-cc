use asm::assemble_file_to_hex;
use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn our_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/add.asm");
    let ours = assemble_file_to_hex(&device::PIC16F877A, src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/add_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", "add.asm", "-o", "add_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success());
    let theirs = std::fs::read_to_string(format!("{dir}/add_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x20] = 5;
    p.run(1000);
    assert_eq!(p.ram()[0x21], 6);
    assert!(p.halted());
}
