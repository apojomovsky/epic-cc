use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn agrees_with_gpasm_assembled_program() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", "fib.asm", "-o", "fib.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(
        out.status.success(),
        "gpasm: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let hex = std::fs::read_to_string(format!("{dir}/fib.hex")).expect("read hex");
    let mut p = Pic14::new(parse_hex(&hex));
    p.ram_mut()[0x20] = 0x12;
    p.ram_mut()[0x21] = 0x34;
    p.run(1000);
    assert_eq!(p.ram()[0x22], 0x46); // 0x12 + 0x34 = 0x46
    assert!(p.halted());
}
