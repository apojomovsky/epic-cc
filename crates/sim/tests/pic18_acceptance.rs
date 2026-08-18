use pic14_sim::{parse_hex_pic18, Pic18};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn hand_written_pic18_program_matches_gpasm_and_runs_correctly() {
    let src = include_str!("fixtures/pic18_acceptance.asm");
    let ours = asm::assemble_file_to_hex(&device::PIC18F4550, src);

    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/pic18_acceptance_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p18f4550", "pic18_acceptance.asm", "-o", "pic18_acceptance_gpasm.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));
    let theirs = std::fs::read_to_string(format!("{dir}/pic18_acceptance_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");

    let mut p = Pic18::new(parse_hex_pic18(&ours));
    p.run(1000);
    assert!(p.halted(), "the program must reach SLEEP within 1000 steps");
    // 0x20 = 5 (stable, never touched again); 0x21 = 5+7 = 12, then the
    // DECFSZ/BRA loop spins it down to 0 (12 iterations); MOVFF copies the
    // stable 0x20 (5) into 0x23; `double` reads 0x23, adds it to itself
    // (5+5=10, written back to 0x23) and returns that in W; W is stored
    // back to 0x23 (a no-op, already 10); MOVLB 1 + MOVWF 0x20,B writes W
    // (10) to the banked physical address 0x120; finally W is stored to
    // 0x24 before SLEEP halts the simulator.
    assert_eq!(p.ram()[0x21], 0, "the loop counter reaches zero");
    assert_eq!(p.ram()[0x23], 10, "double(5) = 10");
    assert_eq!(p.ram()[0x120], 10, "banked write via MOVLB 1 / MOVWF 0x20,B");
    assert_eq!(p.ram()[0x24], 10, "W (10) stored to 0x24 right before SLEEP");
}
