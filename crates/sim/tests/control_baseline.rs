//! Debugger control surface over `PicBaseline` (epic-cc#1313):
//! `run_until` stop reasons, register round-trips, flat physical memory
//! read/write, and program-flash reads. Hand-assembled 12-bit words:
//! MOVLW k is 0xC00|k, MOVWF f is 0x020|f, GOTO a is 0xA00|(a&0x1FF).

use pic14_sim::{PicBaseline, StopReason};

fn sim(prog: Vec<u16>) -> PicBaseline<'static> {
    PicBaseline::with_device(&device::PIC12F509, prog)
}

//   0: MOVLW 0x11
//   1: MOVWF 0x10
//   2: GOTO 0x02 (spins here forever)
fn spin_program() -> Vec<u16> {
    vec![0xC11, 0x030, 0xA02]
}

//   0: MOVLW 0x55
//   1: MOVWF 0x10
//   2: NOP (runs off the end after this word)
fn short_program() -> Vec<u16> {
    vec![0xC55, 0x030, 0x000]
}

#[test]
fn run_until_hits_the_target_exactly() {
    let mut p = sim(short_program());
    assert_eq!(p.run_until(1, 10), StopReason::Reached);
    assert_eq!(p.pc(), 1);
    assert_eq!(p.w(), 0x55, "word 0 executed, word 1 has not");
    p.step();
    assert_eq!(p.ram()[0x10], 0x55, "resuming runs the target word");
}

#[test]
fn run_until_reports_halted_when_the_target_never_hits() {
    let mut p = sim(short_program());
    assert_eq!(p.run_until(0xFF, 100), StopReason::Halted);
    assert!(p.halted());
}

#[test]
fn run_until_stops_at_the_cap_inside_the_spin() {
    let mut p = sim(spin_program());
    assert_eq!(p.run_until(0xFF, 7), StopReason::Capped);
    assert_eq!(p.pc(), 2, "stopped inside the spin loop");
}

#[test]
fn register_writes_round_trip() {
    let mut p = sim(spin_program());
    p.set_w(0x42);
    assert_eq!(p.w(), 0x42, "W write round-trips");
    // A PC write resumes from the written address.
    p.set_pc(1);
    p.step();
    assert_eq!(p.pc(), 2);
    assert_eq!(p.ram()[0x10], 0x42, "resumed mid-program and stored W");
    // Register reads expose the file image.
    assert_eq!(p.status(), 0);
    assert_eq!(p.fsr(), 0);
    assert_eq!(p.pcl(), 2, "PCL reads the low byte of the PC");
    // FSR bit 5 pages the bank; PCL writes page the PC through PA0.
    p.ram_mut()[0x04] = 0x20;
    assert_eq!(p.bank(), 1);
    p.ram_mut()[0x03] = 0x20;
    p.set_pcl(0x34);
    assert_eq!(p.pc(), 0x234, "PC<9> from PA0, PC<8> forced to 0");
}

#[test]
fn memory_read_write_is_flat_and_physical() {
    let mut p = sim(short_program());
    p.write_mem(0x10, &[1, 2, 3]);
    assert_eq!(p.read_mem(0x10, 3), vec![1, 2, 3]);
    assert_eq!(&p.ram()[0x10..0x13], &[1, 2, 3]);
    // Bank selection does not move a physical debugger span.
    p.ram_mut()[0x04] = 0x20;
    assert_eq!(p.read_mem(0x10, 1), vec![1]);
}

#[test]
fn program_flash_reads_by_word_index() {
    let p = sim(short_program());
    assert_eq!(p.read_prog_word(0), Some(0xC55));
    assert_eq!(p.read_prog_word(2), Some(0x000));
    assert_eq!(p.read_prog_word(3), None);
}
