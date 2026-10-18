//! Timer0 counts cross-checked against gpsim 0.31.0, driven as an
//! external process the way `gpasm_cross.rs` drives gpasm (GPL boundary:
//! never linked, only executed). gpsim `step N` advances N instruction
//! cycles, so our sim runs to the same cycle count before comparing.
use pic14_sim::{parse_hex, Pic14, PIC14_OPTION_REG, PIC14_TMR0};
use std::process::Command;

const INTCON: usize = 0x0B;
// POR OPTION_REG is all ones (external clock, Timer0 idle); the sim
// zerofills RAM, so the harness establishes it before stepping.
const POR_OPTION: u8 = 0xFF;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

fn gpsim() -> String {
    std::env::var("PIC8_GPSIM").unwrap_or_else(|_| "gpsim".into())
}

fn scratch_dir(case: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("epic_tmr0_{}_{}", case, std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    std::fs::write(dir.join("timer0.asm"), include_str!("fixtures/timer0.asm"))
        .expect("stage fixture");
    dir
}

fn assemble(dir: &std::path::Path) {
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", "timer0.asm", "-o", "timer0.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(
        out.status.success(),
        "gpasm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn find_line<'a>(output: &'a str, pred: impl Fn(&str) -> bool) -> &'a str {
    output
        .lines()
        .map(str::trim)
        .find(|l| pred(l))
        .unwrap_or_else(|| panic!("wanted line missing in gpsim output:\n{output}"))
}

fn read_reg(output: &str, name: &str) -> u8 {
    // Last match wins: the pre-step preset assignment may echo above.
    let prefix = format!("{name} = 0x");
    let line = output
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with(&prefix))
        .next_back()
        .unwrap_or_else(|| panic!("no {name} line in gpsim output:\n{output}"));
    u8::from_str_radix(line[prefix.len()..].trim(), 16)
        .unwrap_or_else(|_| panic!("bad {name} line: {line}"))
}

fn read_cycles(output: &str) -> u64 {
    let line = find_line(output, |l| {
        l.split('=').count() == 2 && l.split('=').next().unwrap().trim().parse::<u64>().is_ok()
    });
    line.split('=').next().unwrap().trim().parse().unwrap()
}

/// Run the fixture under gpsim for `cycles` with TMR0 preset to `start`,
/// returning gpsim's (TMR0, INTCON).
fn run_gpsim(dir: &std::path::Path, start: u8, cycles: u64) -> (u8, u8) {
    let script = format!(
        "processor p16f877a\nload timer0.cod\np16f877a.tmr0 = 0x{start:02x}\nstep {cycles}\n\
         p16f877a.tmr0\np16f877a.intcon\ncycles\nquit\n"
    );
    std::fs::write(dir.join("run.stc"), script).expect("write stc script");
    let out = Command::new(gpsim())
        .args(["-i", "-c", "run.stc"])
        .current_dir(dir)
        .output()
        .expect("run gpsim");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "gpsim failed:\n{text}");
    assert_eq!(read_cycles(&text), cycles, "gpsim cycle count:\n{text}");
    (read_reg(&text, "tmr0"), read_reg(&text, "intcon"))
}

/// Run our sim over the same gpasm HEX to the same cycle count.
fn run_ours(dir: &std::path::Path, start: u8, cycles: u64) -> (u8, u8) {
    let hex = std::fs::read_to_string(dir.join("timer0.hex")).expect("read hex");
    let mut p = Pic14::new(parse_hex(&hex));
    p.set_timer0_enabled(true);
    p.ram_mut()[PIC14_OPTION_REG] = POR_OPTION;
    p.ram_mut()[PIC14_TMR0] = start;
    while p.cycles() < cycles {
        p.step();
    }
    assert_eq!(p.cycles(), cycles, "overshot the cycle target");
    (p.ram()[PIC14_TMR0], p.ram()[INTCON])
}

fn check_case(case: &str, start: u8, cycles: u64) {
    let dir = scratch_dir(case);
    assemble(&dir);
    let (want_tmr0, want_intcon) = run_gpsim(&dir, start, cycles);
    let (got_tmr0, got_intcon) = run_ours(&dir, start, cycles);
    assert_eq!(got_tmr0, want_tmr0, "TMR0 after {cycles} cycles");
    assert_eq!(
        got_intcon, want_intcon,
        "INTCON (T0IF is bit 2) after {cycles} cycles"
    );
}

#[test]
fn fixed_prescaler_matches_gpsim() {
    check_case("fixed", 0x00, 100);
}

#[test]
fn overflow_start_matches_gpsim() {
    check_case("overflow", 0xFF, 6);
}
