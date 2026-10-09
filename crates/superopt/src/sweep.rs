//! Opcode-level MPLAB SIM differential sweep (epic-cc#819).
//!
//! Each lane replays one ALU opcode across entry-W, operand, and
//! entry-STATUS corners on hardware and in-tree, so the sim cannot share
//! the compiler's wrong belief about an opcode the way epic-cc#632 did.
//! Four shards (PIC18 arithmetic, PIC18 logic+rotates, PIC14 split in
//! two) keep CI wall time flat; every mismatch log names each failing
//! lane via [`mdb::Batch::tags`].

use crate::mdb::{Batch, POISON};
use crate::{Candidate, STATUS_ADDR};
use pic14_sim::{Pic14, Pic18};

/// Which core a sweep shard replays on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SweepDevice {
    Pic18,
    Pic14,
}

/// One replay case: entry `W`, entry `STATUS` corners, and RAM pokes.
/// `entry_status` is a full STATUS value on PIC18; on PIC14 only its
/// C/DC/Z bits take effect (RP bits stay clear so direct operands keep
/// addressing bank 0, TO/PD are never written and are masked on
/// capture). Every lane watches exactly one file address and must poke
/// it: an unpoked watch reads sim-poison in-tree against hardware POR
/// on silicon, and both builders refuse such a case outright. Each
/// case occupies the same three slots (`W`, `STATUS`, watched address).
#[derive(Clone)]
pub struct SweepCase {
    pub entry_w: u8,
    pub entry_status: u8,
    pub pokes: Vec<(usize, u8)>,
}

/// One opcode lane: a single instruction plus its tier case lists.
pub struct Lane {
    pub name: &'static str,
    pub candidate: fn() -> Candidate,
    pub pr_cases: fn() -> Vec<SweepCase>,
    pub nightly_cases: fn() -> Vec<SweepCase>,
}

/// One CI matrix shard: a device plus its lanes.
pub struct SweepSpec {
    pub name: &'static str,
    pub device: SweepDevice,
    pub lanes: fn() -> Vec<Lane>,
    pub chunk_cases: usize,
}

/// One flattened replay unit: a lane's candidate over one case.
#[derive(Clone)]
pub struct SweepItem {
    pub lane: &'static str,
    pub candidate: Candidate,
    pub case: SweepCase,
}

/// The file address every lane operates on (bank 0 GPR on both cores).
pub const LANE_ADDR: usize = 0x20;
/// Operand corners: zero, one, sign edge, extremes. A differential
/// miss is systematic, so edges plus a deterministic stride (nightly)
/// cover it; full bytes stay sim-side.
const OPERAND_CORNERS: &[u8] = &[0x00, 0x01, 0x7F, 0x80, 0xFE, 0xFF];
/// Entry-STATUS corners: clear, carry set, N/OV/Z set. No ALU op reads
/// Z/N/OV, so the third variant proves no hidden dependence on them.
const STATUS_CORNERS: &[u8] = &[0x00, 0x01, 0x1C];
/// DAW reads C and DC, so it crosses those two bits explicitly.
const DAW_STATUS: &[u8] = &[0x00, 0x01, 0x02, 0x03];
/// `W` values for lanes that ignore `W`: both extremes prove it.
const W_PAIR: &[u8] = &[0x00, 0xFF];
/// PIC14 STATUS bits the sweep sets up (C, DC, Z); RP/TO/PD/IRP are
/// never written by the setup sequence.
const PIC14_STATUS_BITS: u8 = 0x07;
/// TO/PD mask applied to the captured STATUS on both executors: MPLAB
/// SIM reports POR-or-WDT values there nondeterministically, so the
/// program itself clears them and sim and hardware stay comparable.
const PIC14_STATUS_MASK: u8 = 0xE7;
/// Proof sentinel the PIC14 program writes at startup. PIC14 has no
/// POR-latched readable register in the sim (RAM starts zeroed, unlike
/// the PIC18 POR table), so a self-written sentinel proves the session
/// actually ran instead.
const PIC14_PROOF_ADDR: usize = 0x2E;
const PIC14_PROOF_VALUE: u8 = 0x3C;
const PIC14_WORK_GUARD: usize = 0x21;
const PIC14_TABLE_GUARD: usize = 0x2F;
const PIC14_TABLE_START: usize = 0x30;
const PIC14_MAX_WORDS: usize = 0x800;
const PIC18_MAX_TABLE: usize = 0x7F0;
const PIC18_FLASH_WORDS: usize = 16384;
const MAX_STEPS: usize = 64;

/// Byte-operand lanes: `W` corners crossed with `F` corners and entry
/// STATUS. Covers every (borrow/no-borrow, carry/no-carry) relation.
fn byte_cases() -> Vec<SweepCase> {
    let mut out = Vec::new();
    for &w in OPERAND_CORNERS {
        for &f in OPERAND_CORNERS {
            for &s in STATUS_CORNERS {
                out.push(SweepCase {
                    entry_w: w,
                    entry_status: s,
                    pokes: vec![(LANE_ADDR, f)],
                });
            }
        }
    }
    out
}

/// Lanes that ignore `W` (increments, complements, moves, rotates on
/// PIC18): `F` corners with both `W` extremes prove the independence.
fn fonly_cases() -> Vec<SweepCase> {
    let mut out = Vec::new();
    for &f in OPERAND_CORNERS {
        for &w in W_PAIR {
            for &s in STATUS_CORNERS {
                out.push(SweepCase {
                    entry_w: w,
                    entry_status: s,
                    pokes: vec![(LANE_ADDR, f)],
                });
            }
        }
    }
    out
}

/// Literal lanes (`W` op `k`): the immediate is baked into the lane,
/// so cases cross `W` and `F` corners with entry STATUS. The `F` poke
/// is load-bearing, not coverage: an unpoked watched address reads
/// sim-poison in-tree against hardware POR on silicon, so every watch
/// must be poked (the same rule `mdb`'s guards rely on).
fn lit_cases() -> Vec<SweepCase> {
    let mut out = Vec::new();
    for &w in OPERAND_CORNERS {
        for &f in OPERAND_CORNERS {
            for &s in STATUS_CORNERS {
                out.push(SweepCase {
                    entry_w: w,
                    entry_status: s,
                    pokes: vec![(LANE_ADDR, f)],
                });
            }
        }
    }
    out
}

/// Deterministic stride over the full byte for nightly: catches a
/// value-dependent divergence the six corners miss, without paying
/// for 256 values per lane.
fn stride_cases(poke_f: bool) -> Vec<SweepCase> {
    let mut out = Vec::new();
    for i in 0..32u16 {
        let w = (i * 8 + 3) as u8;
        let f = (i * 8 + 5) as u8;
        for &s in &[0x00u8, 0x01] {
            let pokes = if poke_f {
                vec![(LANE_ADDR, f)]
            } else {
                Vec::new()
            };
            out.push(SweepCase {
                entry_w: w,
                entry_status: s,
                pokes,
            });
        }
    }
    out
}

fn byte_nightly() -> Vec<SweepCase> {
    let mut cases = byte_cases();
    cases.extend(stride_cases(true));
    cases
}

fn fonly_nightly() -> Vec<SweepCase> {
    let mut cases = fonly_cases();
    cases.extend(stride_cases(true));
    cases
}

fn lit_nightly() -> Vec<SweepCase> {
    let mut cases = lit_cases();
    cases.extend(stride_cases(true));
    cases
}

fn daw_cases() -> Vec<SweepCase> {
    let mut out = Vec::new();
    for &w in &[0x00u8, 0x09, 0x0A, 0x10, 0x99, 0x9A, 0xA0, 0xFF] {
        for &f in &[0x00u8, 0xFF] {
            for &s in DAW_STATUS {
                out.push(SweepCase {
                    entry_w: w,
                    entry_status: s,
                    pokes: vec![(LANE_ADDR, f)],
                });
            }
        }
    }
    out
}

fn daw_nightly() -> Vec<SweepCase> {
    let mut cases = daw_cases();
    for i in 0..32u16 {
        let w = (i * 8 + 3) as u8;
        for &f in &[0x00u8, 0xFF] {
            for &s in DAW_STATUS {
                cases.push(SweepCase {
                    entry_w: w,
                    entry_status: s,
                    pokes: vec![(LANE_ADDR, f)],
                });
            }
        }
    }
    cases
}

/// Rig canary: `MOVLW` overwrites `W` unconditionally, so any harness
/// breakage (assemble, emit, parse, compare) fails this lane first.
fn canary_cases() -> Vec<SweepCase> {
    let mut out = Vec::new();
    for &(w, s) in &[(0x00u8, 0x00u8), (0xFF, 0x01)] {
        for &f in &[0x00u8, 0xFF] {
            out.push(SweepCase {
                entry_w: w,
                entry_status: s,
                pokes: vec![(LANE_ADDR, f)],
            });
        }
    }
    out
}

macro_rules! lane {
    ($name:ident, $candidate:expr, $pr:expr, $nightly:expr) => {
        Lane {
            name: stringify!($name),
            candidate: $candidate,
            pr_cases: $pr,
            nightly_cases: $nightly,
        }
    };
}

macro_rules! single {
    ($line:expr) => {
        || vec![$line]
    };
}

fn pic18_arith_lanes() -> Vec<Lane> {
    vec![
        lane!(addwf, single!("addwf 0x020,F,A"), byte_cases, byte_nightly),
        lane!(
            addwfc,
            single!("addwfc 0x020,F,A"),
            byte_cases,
            byte_nightly
        ),
        lane!(subwf, single!("subwf 0x020,F,A"), byte_cases, byte_nightly),
        lane!(
            subwfb,
            single!("subwfb 0x020,F,A"),
            byte_cases,
            byte_nightly
        ),
        lane!(
            subfwb,
            single!("subfwb 0x020,F,A"),
            byte_cases,
            byte_nightly
        ),
        lane!(addlw_k01, single!("addlw 0x01"), lit_cases, lit_nightly),
        lane!(addlw_kFF, single!("addlw 0xFF"), lit_cases, lit_nightly),
        lane!(sublw_k01, single!("sublw 0x01"), lit_cases, lit_nightly),
        lane!(sublw_kFF, single!("sublw 0xFF"), lit_cases, lit_nightly),
        lane!(incf, single!("incf 0x020,F,A"), fonly_cases, fonly_nightly),
        lane!(decf, single!("decf 0x020,F,A"), fonly_cases, fonly_nightly),
        lane!(negf, single!("negf 0x020,A"), fonly_cases, fonly_nightly),
        lane!(daw, single!("daw"), daw_cases, daw_nightly),
    ]
}

fn pic18_logic_lanes() -> Vec<Lane> {
    vec![
        lane!(andwf, single!("andwf 0x020,F,A"), byte_cases, byte_nightly),
        lane!(iorwf, single!("iorwf 0x020,F,A"), byte_cases, byte_nightly),
        lane!(xorwf, single!("xorwf 0x020,F,A"), byte_cases, byte_nightly),
        lane!(andlw_k55, single!("andlw 0x55"), lit_cases, lit_nightly),
        lane!(iorlw_k55, single!("iorlw 0x55"), lit_cases, lit_nightly),
        lane!(xorlw_k55, single!("xorlw 0x55"), lit_cases, lit_nightly),
        lane!(comf, single!("comf 0x020,F,A"), fonly_cases, fonly_nightly),
        lane!(rlcf, single!("rlcf 0x020,F,A"), fonly_cases, fonly_nightly),
        lane!(rrcf, single!("rrcf 0x020,F,A"), fonly_cases, fonly_nightly),
        lane!(
            rlncf,
            single!("rlncf 0x020,F,A"),
            fonly_cases,
            fonly_nightly
        ),
        lane!(
            rrncf,
            single!("rrncf 0x020,F,A"),
            fonly_cases,
            fonly_nightly
        ),
        lane!(movf, single!("movf 0x020,F,A"), fonly_cases, fonly_nightly),
        lane!(
            swapf,
            single!("swapf 0x020,F,A"),
            fonly_cases,
            fonly_nightly
        ),
        lane!(movlw, single!("movlw 0xA5"), canary_cases, canary_cases),
    ]
}

fn pic14_arith_lanes() -> Vec<Lane> {
    vec![
        lane!(addwf, single!("addwf 0x20, F"), byte_cases, byte_nightly),
        lane!(subwf, single!("subwf 0x20, F"), byte_cases, byte_nightly),
        lane!(addlw_k01, single!("addlw 0x01"), lit_cases, lit_nightly),
        lane!(addlw_kFF, single!("addlw 0xFF"), lit_cases, lit_nightly),
        lane!(sublw_k01, single!("sublw 0x01"), lit_cases, lit_nightly),
        lane!(sublw_kFF, single!("sublw 0xFF"), lit_cases, lit_nightly),
        lane!(incf, single!("incf 0x20, F"), fonly_cases, fonly_nightly),
        lane!(decf, single!("decf 0x20, F"), fonly_cases, fonly_nightly),
    ]
}

fn pic14_logic_lanes() -> Vec<Lane> {
    vec![
        lane!(andwf, single!("andwf 0x20, F"), byte_cases, byte_nightly),
        lane!(iorwf, single!("iorwf 0x20, F"), byte_cases, byte_nightly),
        lane!(xorwf, single!("xorwf 0x20, F"), byte_cases, byte_nightly),
        lane!(andlw_k55, single!("andlw 0x55"), lit_cases, lit_nightly),
        lane!(iorlw_k55, single!("iorlw 0x55"), lit_cases, lit_nightly),
        lane!(xorlw_k55, single!("xorlw 0x55"), lit_cases, lit_nightly),
        lane!(comf, single!("comf 0x20, F"), fonly_cases, fonly_nightly),
        lane!(rlf, single!("rlf 0x20, F"), fonly_cases, fonly_nightly),
        lane!(rrf, single!("rrf 0x20, F"), fonly_cases, fonly_nightly),
        lane!(movf, single!("movf 0x20, F"), fonly_cases, fonly_nightly),
        lane!(swapf, single!("swapf 0x20, F"), fonly_cases, fonly_nightly),
        lane!(clrf, single!("clrf 0x20"), fonly_cases, fonly_nightly),
        lane!(clrw, single!("clrw"), lit_cases, lit_nightly),
        lane!(movlw, single!("movlw 0xA5"), canary_cases, canary_cases),
    ]
}

/// The four CI matrix shards.
pub fn all_sweeps() -> Vec<SweepSpec> {
    vec![
        SweepSpec {
            name: "pic18-arith",
            device: SweepDevice::Pic18,
            lanes: pic18_arith_lanes,
            chunk_cases: 400,
        },
        SweepSpec {
            name: "pic18-logic",
            device: SweepDevice::Pic18,
            lanes: pic18_logic_lanes,
            chunk_cases: 400,
        },
        SweepSpec {
            name: "pic14-arith",
            device: SweepDevice::Pic14,
            lanes: pic14_arith_lanes,
            chunk_cases: 72,
        },
        SweepSpec {
            name: "pic14-logic",
            device: SweepDevice::Pic14,
            lanes: pic14_logic_lanes,
            chunk_cases: 72,
        },
    ]
}

/// Flatten a shard's tier into replay units, lane by lane.
pub fn flatten(spec: &SweepSpec, tier: &str) -> Option<Vec<SweepItem>> {
    let lanes = (spec.lanes)();
    let mut items = Vec::new();
    for lane in &lanes {
        let cases = match tier {
            "pr" => (lane.pr_cases)(),
            "nightly" => (lane.nightly_cases)(),
            _ => return None,
        };
        for case in cases {
            items.push(SweepItem {
                lane: lane.name,
                candidate: (lane.candidate)(),
                case,
            });
        }
    }
    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

/// MPLAB part for a shard's device, selected by the runner script.
pub fn part_name(device: SweepDevice) -> &'static str {
    match device {
        SweepDevice::Pic18 => "PIC18F4550",
        SweepDevice::Pic14 => "PIC16F877A",
    }
}

const PIC18_OUT_BASE: usize = 0x100;
const PIC18_WREG: usize = 0xFE8;
const PIC18_STATUS: usize = 0xFD8;
const PIC18_SFR_FLOOR: usize = 0xF80;

/// STATUS bits the gate compares per lane. MPLAB SIM 6.35 models the
/// carry-reading add/sub flags wrongly (hand-probed against boolean
/// truth, triangulated with gpsim): ADDWFC drops the (W+C) low-nibble
/// carry from DC, and SUBWFB/SUBFWB miscompute DC+OV with borrow-in.
/// Those bits are masked where proven wrong; every other bit on every
/// lane still compares, and the masked bits stay pinned by in-tree
/// sim tests.
pub fn status_mask(lane: &str) -> u8 {
    match lane {
        "addwfc" => 0xFD,
        "subwfb" | "subfwb" => 0xF5,
        _ => 0xFF,
    }
}

/// Replay sweep items on PIC18, mirroring `mdb::build_batch` line for
/// line (proof register, poisoned guards, per-case `W`/`STATUS`/watched
/// slots) with the lane name as each read's tag.
pub fn build_sweep_18(items: &[SweepItem]) -> Batch {
    let mut src = String::from("goto start\nstart:\n");
    let mut reads = vec![crate::mdb::PROOF_ADDR];
    let mut expected = vec![crate::mdb::PROOF_POR];
    let mut tags = vec![String::from("proof")];
    let poked: Vec<usize> = items
        .iter()
        .flat_map(|it| it.case.pokes.iter().map(|(a, _)| *a))
        .collect();
    let mut guards: Vec<usize> = Vec::new();
    for g in [LANE_ADDR.wrapping_sub(1), LANE_ADDR + 1] {
        if g < PIC18_SFR_FLOOR && !poked.contains(&g) && !guards.contains(&g) {
            guards.push(g);
        }
    }
    guards.sort_unstable();
    let slots = 3;
    let table_end = PIC18_OUT_BASE + items.len() * slots;
    assert!(
        table_end < PIC18_MAX_TABLE,
        "sweep: batch table ends at 0x{table_end:03X}, past RAM"
    );
    for g in [PIC18_OUT_BASE.wrapping_sub(1), table_end] {
        if !guards.contains(&g) {
            guards.push(g);
        }
    }
    src.push_str("movlw 0xA5\n");
    for &g in &guards {
        if g < 0x80 {
            src.push_str(&format!("movwf 0x{g:03X},A\n"));
        } else {
            src.push_str(&format!("movff 0x{PIC18_WREG:03X},0x{g:03X}\n"));
        }
    }
    for (i, it) in items.iter().enumerate() {
        for line in &it.candidate {
            assert!(!line.contains(':'), "sweep lanes are label-free single ops");
        }
        // An unpoked watch reads sim-poison in-tree against hardware
        // POR on silicon: every lane must poke what it watches.
        assert!(
            it.case.pokes.iter().any(|&(a, _)| a == LANE_ADDR),
            "sweep lane {} leaves its watch unpoked",
            it.lane
        );
        for &(addr, val) in &it.case.pokes {
            src.push_str(&format!("movlw 0x{val:02X}\n"));
            if addr < 0x80 || addr >= PIC18_SFR_FLOOR {
                src.push_str(&format!("movwf 0x{addr:03X},A\n"));
            } else {
                src.push_str(&format!("movff 0x{PIC18_WREG:03X},0x{addr:03X}\n"));
            }
        }
        src.push_str(&format!("movlw 0x{:02X}\n", it.case.entry_status));
        src.push_str(&format!("movwf 0x{PIC18_STATUS:03X},A\n"));
        src.push_str(&format!("movlw 0x{:02X}\n", it.case.entry_w));
        for line in &it.candidate {
            src.push_str(line);
            src.push('\n');
        }
        let slot = PIC18_OUT_BASE + i * slots;
        src.push_str(&format!("movff 0x{PIC18_WREG:03X},0x{slot:03X}\n"));
        let mask = status_mask(it.lane);
        if mask == 0xFF {
            src.push_str(&format!("movff 0x{PIC18_STATUS:03X},0x{:03X}\n", slot + 1));
        } else {
            // Quirk-masked lanes capture STATUS through W with the
            // proven-wrong bits cleared on both executors (same trick
            // as the PIC14 TO/PD mask). `movf` runs after the W save,
            // so its Z side effect is already recorded.
            src.push_str(&format!("movf 0x{PIC18_STATUS:03X},W,A\n"));
            src.push_str(&format!("andlw 0x{mask:02X}\n"));
            src.push_str(&format!("movff 0x{PIC18_WREG:03X},0x{:03X}\n", slot + 1));
        }
        src.push_str(&format!("movff 0x{LANE_ADDR:03X},0x{:03X}\n", slot + 2));
    }
    src.push_str("spin:\ngoto spin\n");
    for &g in &guards {
        reads.push(g);
        expected.push(POISON);
        tags.push(String::from("guard"));
    }
    for (i, it) in items.iter().enumerate() {
        let slot = PIC18_OUT_BASE + i * slots;
        let vals = run_expected_18(it).expect("sweep case must verify in-sim");
        reads.push(slot);
        reads.push(slot + 1);
        reads.push(slot + 2);
        expected.push(vals.0);
        expected.push(vals.1);
        expected.push(vals.2);
        tags.push(it.lane.to_string());
        tags.push(it.lane.to_string());
        tags.push(it.lane.to_string());
    }
    let words = asm::assemble_pic18(&src);
    assert!(
        words.len() < PIC18_FLASH_WORDS,
        "sweep: {} items exceed 18F4550 flash",
        items.len()
    );
    let instrs = src
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_end().ends_with(':'))
        .count();
    Batch {
        src,
        reads,
        expected,
        tags,
        stepi: instrs + 2000,
    }
}

/// Expected `(W, STATUS, watched)` for one PIC18 replay from the
/// in-tree simulator, mirroring `mdb`'s setup exactly.
fn run_expected_18(it: &SweepItem) -> Option<(u8, u8, u8)> {
    let mut src = String::new();
    for line in &it.candidate {
        src.push_str(line);
        src.push('\n');
    }
    src.push_str("sleep\n");
    let words = asm::assemble_pic18(&src);
    let sleep_addr = ((words.len() - 1) * 2) as u32;
    let mut sim = Pic18::new(words);
    sim.ram_mut().fill(POISON);
    sim.ram_mut()[STATUS_ADDR] = it.case.entry_status;
    sim.set_w(it.case.entry_w);
    for &(addr, val) in &it.case.pokes {
        sim.ram_mut()[addr] = val;
    }
    sim.run(MAX_STEPS);
    if !(sim.halted() && sim.pc() == sleep_addr) {
        return None;
    }
    Some((
        sim.w(),
        sim.ram()[STATUS_ADDR] & status_mask(it.lane),
        sim.ram()[LANE_ADDR],
    ))
}

/// GPR ranges backing the PIC14 output table, derived from the
/// device map so a memory-map edit fails here instead of silently
/// addressing SFRs. Each bank contributes all but its last byte;
/// that byte is the bank run's poisoned guard.
fn pic14_table_ranges() -> (Vec<usize>, Vec<usize>) {
    let device = device::resolve("16F877A").expect("16F877A in device map");
    let mut usable = Vec::new();
    let mut guards = Vec::new();
    for &(lo, hi) in device.ram_banks {
        let lo = lo as usize;
        let hi = hi as usize;
        let start = lo.max(PIC14_TABLE_START);
        assert!(start < hi, "sweep: 16F877A bank {lo:#X} has no table room");
        for addr in start..hi {
            usable.push(addr);
        }
        guards.push(hi);
    }
    (usable, guards)
}

/// Select a PIC14 RAM bank: RP1:RP0 are STATUS bits 6:5, and
/// `bcf`/`bsf` on them affect no flags.
fn sel_bank(src: &mut String, cur: &mut u8, bank: u8) {
    if *cur == bank {
        return;
    }
    src.push_str(if bank & 1 != 0 {
        "bsf 0x03, 5\n"
    } else {
        "bcf 0x03, 5\n"
    });
    src.push_str(if bank & 2 != 0 {
        "bsf 0x03, 6\n"
    } else {
        "bcf 0x03, 6\n"
    });
    *cur = bank;
}

/// Replay sweep items on PIC14. Same contract as the PIC18 builder:
/// proof sentinel, poisoned guards, per-case `W`/masked-`STATUS`/
/// watched slots, lane tags. Stores go through RP1:RP0 bank selects
/// into a table spanning banks 0-3; the entry-STATUS setup and the
/// restore afterwards keep RP bits clear so the lane itself always
/// addresses bank 0.
pub fn build_sweep_14(items: &[SweepItem]) -> Batch {
    let (usable, bank_guards) = pic14_table_ranges();
    assert!(
        items.len() * 3 <= usable.len(),
        "sweep: {} items need {} slots, table holds {}",
        items.len(),
        items.len() * 3,
        usable.len()
    );
    let mut src = String::from("goto start\nstart:\n");
    let mut reads = vec![PIC14_PROOF_ADDR];
    let mut expected = vec![PIC14_PROOF_VALUE];
    let mut tags = vec![String::from("proof")];
    let mut guards = vec![PIC14_WORK_GUARD, PIC14_TABLE_GUARD];
    let mut used_banks = vec![false; 4];
    for i in 0..items.len() * 3 {
        used_banks[usable[i] / 0x80] = true;
    }
    for (bank, used) in used_banks.iter().enumerate() {
        if *used {
            guards.push(bank_guards[bank]);
        }
    }
    guards.sort_unstable();
    let mut cur = 0u8;
    src.push_str("movlw 0xA5\n");
    for &g in &guards {
        sel_bank(&mut src, &mut cur, (g / 0x80) as u8);
        src.push_str(&format!("movwf 0x{:02X}\n", g % 0x80));
    }
    src.push_str(&format!("movlw 0x{PIC14_PROOF_VALUE:02X}\n"));
    sel_bank(&mut src, &mut cur, 0);
    src.push_str(&format!("movwf 0x{PIC14_PROOF_ADDR:02X}\n"));
    for (i, it) in items.iter().enumerate() {
        for line in &it.candidate {
            assert!(!line.contains(':'), "sweep lanes are label-free single ops");
        }
        assert!(
            it.case.pokes.iter().any(|&(a, _)| a == LANE_ADDR),
            "sweep lane {} leaves its watch unpoked",
            it.lane
        );
        sel_bank(&mut src, &mut cur, 0);
        for &(addr, val) in &it.case.pokes {
            assert!(addr < 0x80, "sweep pokes stay in bank 0");
            src.push_str(&format!("movlw 0x{val:02X}\n"));
            src.push_str(&format!("movwf 0x{addr:02X}\n"));
        }
        src.push_str(&format!("movlw 0x{:02X}\n", it.case.entry_w));
        for bit in 0..3 {
            let set = it.case.entry_status & (1 << bit) != 0;
            if set {
                src.push_str(&format!("bsf 0x03, {bit}\n"));
            } else {
                src.push_str(&format!("bcf 0x03, {bit}\n"));
            }
        }
        for line in &it.candidate {
            src.push_str(line);
            src.push('\n');
        }
        let base = i * 3;
        let slot_w = usable[base];
        let slot_s = usable[base + 1];
        let slot_f = usable[base + 2];
        sel_bank(&mut src, &mut cur, (slot_w / 0x80) as u8);
        src.push_str(&format!("movwf 0x{:02X}\n", slot_w % 0x80));
        sel_bank(&mut src, &mut cur, 0);
        src.push_str("movf 0x03, W\n");
        let mask = PIC14_STATUS_MASK & status_mask(it.lane);
        src.push_str(&format!("andlw 0x{mask:02X}\n"));
        sel_bank(&mut src, &mut cur, (slot_s / 0x80) as u8);
        src.push_str(&format!("movwf 0x{:02X}\n", slot_s % 0x80));
        sel_bank(&mut src, &mut cur, 0);
        src.push_str(&format!("movf 0x{LANE_ADDR:02X}, W\n"));
        sel_bank(&mut src, &mut cur, (slot_f / 0x80) as u8);
        src.push_str(&format!("movwf 0x{:02X}\n", slot_f % 0x80));
        sel_bank(&mut src, &mut cur, 0);
    }
    // The park loop pets the watchdog: MPLAB SIM resets the part on a
    // WDT timeout during the runner's wall-clock wait, and a reset
    // mid-replay only settles when the program lands back here promptly.
    src.push_str("spin:\nclrwdt\ngoto spin\n");
    for &g in &guards {
        reads.push(g);
        expected.push(POISON);
        tags.push(String::from("guard"));
    }
    for (i, it) in items.iter().enumerate() {
        let base = i * 3;
        let vals = run_expected_14(it).expect("sweep case must verify in-sim");
        reads.push(usable[base]);
        reads.push(usable[base + 1]);
        reads.push(usable[base + 2]);
        expected.push(vals.0);
        expected.push(vals.1);
        expected.push(vals.2);
        tags.push(it.lane.to_string());
        tags.push(it.lane.to_string());
        tags.push(it.lane.to_string());
    }
    let words = asm::assemble(&src);
    assert!(
        words.len() < PIC14_MAX_WORDS,
        "sweep: {} items exceed page 0 (goto cannot leave it)",
        items.len()
    );
    let instrs = src
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_end().ends_with(':'))
        .count();
    Batch {
        src,
        reads,
        expected,
        tags,
        stepi: instrs + 2000,
    }
}

/// Expected `(W, masked STATUS, watched)` for one PIC14 replay.
fn run_expected_14(it: &SweepItem) -> Option<(u8, u8, u8)> {
    let mut src = String::new();
    for line in &it.candidate {
        src.push_str(line);
        src.push('\n');
    }
    src.push_str("sleep\n");
    let words = asm::assemble(&src);
    let sleep_addr = (words.len() - 1) as u16;
    let mut sim = Pic14::new(words);
    sim.ram_mut().fill(POISON);
    sim.ram_mut()[0x03] = it.case.entry_status & PIC14_STATUS_BITS;
    sim.set_w(it.case.entry_w);
    for &(addr, val) in &it.case.pokes {
        sim.ram_mut()[addr] = val;
    }
    sim.run(MAX_STEPS);
    if !(sim.halted() && sim.pc() == sleep_addr) {
        return None;
    }
    Some((
        sim.w(),
        sim.ram()[0x03] & PIC14_STATUS_MASK & status_mask(it.lane),
        sim.ram()[LANE_ADDR],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mdb::compare_batch;

    fn pr_items(name: &str) -> (SweepSpec, Vec<SweepItem>) {
        let spec = all_sweeps().into_iter().find(|s| s.name == name).unwrap();
        let items = flatten(&spec, "pr").unwrap();
        assert!(!items.is_empty(), "{name} pr tier must not be empty");
        (spec, items)
    }

    /// Every chunk CI will emit for a shard builds: same slice, same
    /// builder, same flash and table asserts.
    fn build_chunks_18(spec: &SweepSpec, items: &[SweepItem]) -> Vec<Batch> {
        items.chunks(spec.chunk_cases).map(build_sweep_18).collect()
    }

    fn build_chunks_14(spec: &SweepSpec, items: &[SweepItem]) -> Vec<Batch> {
        items.chunks(spec.chunk_cases).map(build_sweep_14).collect()
    }

    #[test]
    fn pic18_shards_build_and_tag_every_read() {
        for name in ["pic18-arith", "pic18-logic"] {
            let (spec, items) = pr_items(name);
            for batch in build_chunks_18(&spec, &items) {
                assert_eq!(batch.reads.len(), batch.expected.len());
                assert_eq!(batch.reads.len(), batch.tags.len());
                assert_eq!(batch.tags[0], "proof");
                assert_eq!(batch.expected[0], crate::mdb::PROOF_POR);
                let words = asm::assemble_pic18(&batch.src);
                assert!(!words.is_empty());
                let lanes: Vec<_> = items.iter().map(|it| it.lane).collect();
                for tag in batch.tags.iter().skip(1 + 4) {
                    assert!(
                        tag == "guard" || lanes.contains(&tag.as_str()),
                        "stray tag {tag}"
                    );
                }
            }
        }
    }

    #[test]
    fn pic14_shards_build_inside_gpr_and_mask_status() {
        let device = device::resolve("16F877A").unwrap();
        let in_gpr = |addr: usize| {
            device
                .ram_banks
                .iter()
                .any(|&(lo, hi)| addr >= lo as usize && addr <= hi as usize)
        };
        for name in ["pic14-arith", "pic14-logic"] {
            let (spec, items) = pr_items(name);
            for batch in build_chunks_14(&spec, &items) {
                assert_eq!(batch.reads.len(), batch.expected.len());
                assert_eq!(batch.reads.len(), batch.tags.len());
                assert_eq!(batch.expected[0], PIC14_PROOF_VALUE);
                for &addr in &batch.reads {
                    assert!(
                        addr == PIC14_PROOF_ADDR || in_gpr(addr),
                        "read 0x{addr:X} outside GPR"
                    );
                }
                let words = asm::assemble(&batch.src);
                assert!(!words.is_empty());
                assert!(words.len() < PIC14_MAX_WORDS);
            }
        }
        let (_, items) = pr_items("pic14-arith");
        for it in items.iter().take(50) {
            let (_, status, _) = run_expected_14(it).unwrap();
            assert_eq!(status & 0x18, 0, "TO/PD must stay masked");
        }
    }

    #[test]
    fn nightly_tiers_cover_every_lane() {
        for spec in all_sweeps() {
            let items = flatten(&spec, "nightly").unwrap();
            assert!(
                items.len() >= pr_items(spec.name).1.len(),
                "{} nightly < pr",
                spec.name
            );
            let mut lanes: Vec<&str> = items.iter().map(|it| it.lane).collect();
            lanes.sort_unstable();
            lanes.dedup();
            let mut want: Vec<&str> = (spec.lanes)().iter().map(|l| l.name).collect();
            want.sort_unstable();
            assert_eq!(lanes, want, "{} drops a lane", spec.name);
            for chunk in items.chunks(spec.chunk_cases) {
                match spec.device {
                    SweepDevice::Pic18 => build_sweep_18(chunk),
                    SweepDevice::Pic14 => build_sweep_14(chunk),
                };
            }
        }
    }

    #[test]
    fn compare_names_every_failing_lane() {
        let (_, items) = pr_items("pic18-arith");
        let take = |lane: &str| items.iter().find(|it| it.lane == lane).unwrap().clone();
        let pair = vec![take("addwf"), take("subwfb")];
        let batch = build_sweep_18(&pair);
        let mut values = batch.expected.clone();
        let first = batch.tags.iter().position(|t| t == "addwf").unwrap();
        let second = batch.tags.iter().position(|t| t == "subwfb").unwrap();
        values[first] ^= 0xFF;
        values[second] ^= 0xFF;
        let err = compare_batch(&batch, &values).unwrap_err();
        assert!(err.contains("lane addwf"), "missing addwf lane:\n{err}");
        assert!(err.contains("lane subwfb"), "missing subwfb lane:\n{err}");
        assert!(compare_batch(&batch, &batch.expected).is_ok());
    }
}
