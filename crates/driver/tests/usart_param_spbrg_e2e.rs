//! epic-cc#632: the parameterized USART_ComputeSPBRG path.
//!
//! The PIC18 restoring divide subtracts the divisor from the partial
//! remainder with a `SUBWF` / `SUBWFB` borrow chain (`f - W - borrow`). The
//! backend emitted `SUBFWB` for every lane above the first, which computes
//! `W - f - borrow`: the operands reversed. The simulator decoded both
//! opcodes with the same formula, so every sim gate agreed with the wrong
//! asm; MPLAB SIM and real silicon do not. `epic-menu-demo` was the visible
//! casualty, its UART baud crawling to 183 and the capture truncating.
//!
//! The fixture forwards runtime `fosc_hz`/`baud` through an inner call, the
//! shape `epic_serial_init` uses, so whole-program constant folding cannot
//! remove the divide (the control-demo shape folded it away, which is why
//! that gate stayed green).

use std::process::Command;

/// Compile the fixture for PIC18, run it in the PIC18 simulator, and return
/// the resulting `g_out` word with the two SPBRG bytes.
fn run_fixture() -> (u16, u8, u8) {
    let hex = std::env::temp_dir().join(format!("spbrg632-{}.hex", std::process::id()));
    let map = std::env::temp_dir().join(format!("spbrg632-{}.map", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/usart_param_spbrg_main.c",
            "tests/fixtures/usart_param_spbrg.c",
            "-o",
        ])
        .arg(&hex)
        .arg("--map")
        .arg(&map)
        .args(["--device", "18F4550"])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver must compile the parameterized SPBRG path: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = std::fs::read_to_string(&map).expect("read map");
    let addr = |name: &str| -> usize {
        text.lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("global ")
                    .and_then(|r| r.split_once(' '))
                    .filter(|(n, _)| *n == name)
                    .and_then(|(_, a)| usize::from_str_radix(a.trim_start_matches("0x"), 16).ok())
            })
            .unwrap_or_else(|| panic!("{name} not in map:\n{text}"))
    };
    let g_out = addr("g_out");

    let prog = pic14_sim::parse_hex_pic18(&std::fs::read_to_string(&hex).expect("read hex"));
    let mut p = pic14_sim::Pic18::new(prog);
    p.run(200_000);
    assert!(p.halted(), "the fixture must halt");

    let word = p.ram()[g_out] as u16 | ((p.ram()[g_out + 1] as u16) << 8);
    (word, p.ram()[0xFAF], p.ram()[0xFB0])
}

#[test]
fn parameterized_spbrg_computes_1249() {
    let (word, spbrg, spbrgh) = run_fixture();
    assert_eq!(
        word, 1249,
        "compute_spbrg(48 MHz, 9600, async, BRGH high, BRG16 wide) is 0x4E1"
    );
    assert_eq!(
        (spbrg, spbrgh),
        (0xE1, 0x04),
        "SPBRG:SPBRGH must hold the same 1249, not the 0xFFFF sentinel"
    );
}
