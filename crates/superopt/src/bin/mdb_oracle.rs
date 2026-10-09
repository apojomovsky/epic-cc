//! `mdb_oracle` driver (epic-cc#712): emit a hardware replay batch for one
//! oracle spec, or check an MDB session log against its expectation.
//!
//! `emit --spec <name> --tier <pr|nightly> --chunk <i/n> --out-dir <dir>`:
//! writes `prog.asm`, `prog.hex` (`asm::to_hex`, same byte layout
//! `gpasm -a inhx32` produces), `expected.bin`, `reads.txt` (one
//! `0xAAA` address per line, in readback order), `lanes.txt` (the lane
//! tag per read, same order), `device.txt` (the MPLAB part the runner
//! must select), and `stepi.txt`. `--mutate` corrupts only the emitted
//! program, so a subsequent `check` must fail.
//!
//! `count --spec <name> --tier <tier>`: prints `<cases> <chunk_cases>`
//! for the orchestrator's chunk loop.
//!
//! `check --out-dir <dir> --mdb-log <file>`: parses the session log and
//! diffs it against `expected.bin`, failing on any missing, unparsable,
//! or mismatched byte. Every mismatch is attributed to its lane, so one
//! log names each failing opcode lane of a sweep shard explicitly.

use std::process::ExitCode;
use superopt::mdb::{build_batch, compare_batch, parse_xbr, Batch};
use superopt::specs::{all_specs, Spec};
use superopt::sweep::{self, SweepDevice, SweepItem, SweepSpec};
use superopt::Case;

const OUT_BASE: usize = 0x100;

enum SpecKind {
    Seq(Spec),
    Sweep(SweepSpec),
}

fn lookup(name: &str) -> Option<SpecKind> {
    if let Some(spec) = all_specs().into_iter().find(|s| s.name == name) {
        return Some(SpecKind::Seq(spec));
    }
    sweep::all_sweeps()
        .into_iter()
        .find(|s| s.name == name)
        .map(SpecKind::Sweep)
}

fn device_of(kind: &SpecKind) -> &'static str {
    match kind {
        SpecKind::Seq(_) => sweep::part_name(SweepDevice::Pic18),
        SpecKind::Sweep(spec) => sweep::part_name(spec.device),
    }
}

fn tier_cases(spec: &Spec, tier: &str) -> Option<Vec<Case>> {
    match tier {
        "pr" => Some((spec.pr_cases)()),
        "nightly" => Some((spec.nightly_cases)()),
        _ => None,
    }
}

fn write_batch(out_dir: &str, batch: &Batch, device: &str) -> Result<usize, String> {
    let words = match device {
        "PIC18F4550" => asm::assemble_pic18(&batch.src),
        _ => asm::assemble(&batch.src),
    };
    let hex = asm::to_hex(&words);
    std::fs::create_dir_all(out_dir)
        .and_then(|_| std::fs::write(format!("{out_dir}/prog.asm"), &batch.src))
        .and_then(|_| std::fs::write(format!("{out_dir}/prog.hex"), &hex))
        .and_then(|_| std::fs::write(format!("{out_dir}/expected.bin"), &batch.expected))
        .and_then(|_| std::fs::write(format!("{out_dir}/stepi.txt"), batch.stepi.to_string()))
        .and_then(|_| std::fs::write(format!("{out_dir}/device.txt"), format!("{device}\n")))
        .and_then(|_| {
            std::fs::write(
                format!("{out_dir}/reads.txt"),
                batch
                    .reads
                    .iter()
                    .map(|a| format!("0x{a:03X}"))
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n",
            )
        })
        .and_then(|_| std::fs::write(format!("{out_dir}/lanes.txt"), batch.tags.join("\n") + "\n"))
        .map_err(|e| format!("mdb_oracle: cannot write {out_dir}: {e}"))?;
    Ok(words.len())
}

fn build_sweep_batch(device: SweepDevice, items: &[SweepItem]) -> Batch {
    match device {
        SweepDevice::Pic18 => sweep::build_sweep_18(items),
        SweepDevice::Pic14 => sweep::build_sweep_14(items),
    }
}

fn emit(spec_name: &str, tier: &str, chunk: &str, out_dir: &str, mutate: bool) -> ExitCode {
    let Some(kind) = lookup(spec_name) else {
        eprintln!("mdb_oracle: unknown spec {spec_name:?}");
        return ExitCode::from(2);
    };
    let device = device_of(&kind);
    let (chunk_i, chunk_n) = parse_chunk(chunk);
    if chunk_i >= chunk_n || chunk_n == 0 {
        eprintln!("mdb_oracle: bad chunk {chunk:?}");
        return ExitCode::from(2);
    }
    match kind {
        SpecKind::Seq(spec) => {
            let Some(cases) = tier_cases(&spec, tier) else {
                eprintln!("mdb_oracle: unknown tier {tier:?}");
                return ExitCode::from(2);
            };
            let size = cases.len().div_ceil(chunk_n);
            let cases: Vec<Case> = cases.into_iter().skip(chunk_i * size).take(size).collect();
            if cases.is_empty() {
                eprintln!("mdb_oracle: chunk {chunk_i}/{chunk_n} is empty");
                return ExitCode::from(2);
            }
            let candidate = (spec.candidate)();
            // Fail-closed mutation: neuter the first line (minimal
            // sequences are load-bearing throughout) and confirm in-sim
            // that the mutant is actually wrong on exactly this chunk. A
            // mutation that still verifies is a broken proof, not a
            // passing gate.
            let src_candidate = if mutate {
                let mut m = candidate.clone();
                m[0] = "nop";
                if superopt::verify(&m, &cases) {
                    eprintln!("mdb_oracle: mutant still verifies, refusing proof");
                    return ExitCode::from(2);
                }
                m
            } else {
                candidate.clone()
            };
            let batch = build_batch(&src_candidate, &cases, OUT_BASE);
            let expected: Batch = if mutate {
                build_batch(&candidate, &cases, OUT_BASE)
            } else {
                build_batch(&src_candidate, &cases, OUT_BASE)
            };
            emit_batch(out_dir, &batch, &expected, cases.len(), device)
        }
        SpecKind::Sweep(spec) => {
            let Some(items) = sweep::flatten(&spec, tier) else {
                eprintln!("mdb_oracle: unknown tier {tier:?}");
                return ExitCode::from(2);
            };
            let size = items.len().div_ceil(chunk_n);
            let items: Vec<SweepItem> = items.into_iter().skip(chunk_i * size).take(size).collect();
            if items.is_empty() {
                eprintln!("mdb_oracle: chunk {chunk_i}/{chunk_n} is empty");
                return ExitCode::from(2);
            }
            let mut src_items = items.clone();
            // Fail-closed mutation, mirroring the sequence path: neuter
            // the first lane's line and require an observable
            // expectation diff on exactly this chunk below.
            if mutate {
                src_items[0].candidate[0] = "nop";
            }
            let batch = build_sweep_batch(spec.device, &src_items);
            let expected: Batch = if mutate {
                build_sweep_batch(spec.device, &items)
            } else {
                build_sweep_batch(spec.device, &src_items)
            };
            if mutate && batch.expected == expected.expected {
                eprintln!("mdb_oracle: mutant is unobservable on this chunk, refusing proof");
                return ExitCode::from(2);
            }
            emit_batch(out_dir, &batch, &expected, items.len(), device)
        }
    }
}

fn emit_batch(
    out_dir: &str,
    batch: &Batch,
    expected: &Batch,
    n_cases: usize,
    device: &str,
) -> ExitCode {
    // `expected.bin` must come from the unmutated batch: the emitted
    // program carries the mutation, the contract does not.
    let staged = Batch {
        src: batch.src.clone(),
        reads: expected.reads.clone(),
        expected: expected.expected.clone(),
        tags: expected.tags.clone(),
        stepi: batch.stepi,
    };
    let words = match write_batch(out_dir, &staged, device) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    // The mutated program assembled in its own build above (a mutant
    // that does not assemble panics there); its wrongness is proven by
    // the expectation diff in `emit`, not here.
    println!(
        "emit: {n_cases} cases, {words} words, {} reads -> {out_dir}",
        staged.reads.len()
    );
    ExitCode::SUCCESS
}

fn parse_chunk(chunk: &str) -> (usize, usize) {
    let (i, n) = chunk.split_once('/').unwrap_or(("0", "1"));
    (i.parse().unwrap_or(0), n.parse().unwrap_or(1))
}

fn check(out_dir: &str, log_path: &str) -> ExitCode {
    let expected = match std::fs::read(format!("{out_dir}/expected.bin")) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("mdb_oracle: cannot read expected.bin: {e}");
            return ExitCode::FAILURE;
        }
    };
    let reads_txt = match std::fs::read_to_string(format!("{out_dir}/reads.txt")) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mdb_oracle: cannot read reads.txt: {e}");
            return ExitCode::FAILURE;
        }
    };
    let reads: Vec<usize> = reads_txt
        .lines()
        .map(|l| usize::from_str_radix(l.trim_start_matches("0x"), 16).unwrap())
        .collect();
    let tags: Vec<String> = match std::fs::read_to_string(format!("{out_dir}/lanes.txt")) {
        Ok(s) => s.lines().map(str::to_string).collect(),
        Err(e) => {
            eprintln!("mdb_oracle: cannot read lanes.txt: {e}");
            return ExitCode::FAILURE;
        }
    };
    let device = match std::fs::read_to_string(format!("{out_dir}/device.txt")) {
        Ok(s) => s.trim().to_string(),
        Err(e) => {
            eprintln!("mdb_oracle: cannot read device.txt: {e}");
            return ExitCode::FAILURE;
        }
    };
    let log = match std::fs::read_to_string(log_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mdb_oracle: cannot read {log_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !log.contains(&format!("device {device}")) {
        eprintln!("mdb: session did not select {device}");
        return ExitCode::FAILURE;
    }
    let values = match parse_xbr(&log, &reads) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("mdb: {e}");
            return ExitCode::FAILURE;
        }
    };
    let batch = Batch {
        src: String::new(),
        reads,
        expected,
        tags,
        stepi: 0,
    };
    match compare_batch(&batch, &values) {
        Ok(()) => {
            println!("MDB ORACLE PASS: {} bytes agree with sim", values.len());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mdb: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    match args.first().map(String::as_str) {
        Some("emit") => {
            let (Some(s), Some(t), Some(d)) = (get("--spec"), get("--tier"), get("--out-dir"))
            else {
                eprintln!("usage: mdb_oracle emit --spec <name> --tier <pr|nightly> [--chunk <i/n>] --out-dir <dir> [--mutate]");
                return ExitCode::from(2);
            };
            let chunk = get("--chunk").unwrap_or_else(|| "0/1".to_string());
            emit(&s, &t, &chunk, &d, args.iter().any(|a| a == "--mutate"))
        }
        Some("count") => {
            let (Some(s), Some(t)) = (get("--spec"), get("--tier")) else {
                eprintln!("usage: mdb_oracle count --spec <name> --tier <pr|nightly>");
                return ExitCode::from(2);
            };
            let Some(kind) = lookup(&s) else {
                eprintln!("mdb_oracle: unknown spec {s:?}");
                return ExitCode::from(2);
            };
            match kind {
                SpecKind::Seq(spec) => {
                    let Some(cases) = tier_cases(&spec, &t) else {
                        eprintln!("mdb_oracle: unknown tier {t:?}");
                        return ExitCode::from(2);
                    };
                    println!("{} {}", cases.len(), spec.chunk_cases);
                }
                SpecKind::Sweep(spec) => {
                    let Some(items) = sweep::flatten(&spec, &t) else {
                        eprintln!("mdb_oracle: unknown tier {t:?}");
                        return ExitCode::from(2);
                    };
                    println!("{} {}", items.len(), spec.chunk_cases);
                }
            }
            ExitCode::SUCCESS
        }
        Some("check") => {
            let (Some(d), Some(l)) = (get("--out-dir"), get("--mdb-log")) else {
                eprintln!("usage: mdb_oracle check --out-dir <dir> --mdb-log <file>");
                return ExitCode::from(2);
            };
            check(&d, &l)
        }
        _ => {
            eprintln!("usage: mdb_oracle <emit|count|check> ...");
            ExitCode::from(2)
        }
    }
}
