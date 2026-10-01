//! epic-cc#837: the fixed-region RAM count must never undercount the bytes
//! the emitted program touches. `fixed_uses` mirrors isel lowering site by
//! site, so a site missing there would silently shrink the report below
//! what the program uses (the PlatformIO pre-flash check reads it). This
//! test compiles a shape-covering corpus (valued calls, the PIC18
//! const-sub flag chain, `_delay`, memcpy, wide compares, large const
//! tables, ISRs, plus the small-program floor rows) and asserts the
//! reported `fixed:`/`common:` bytes cover every fixed-range reference
//! in the emitted asm. It guards the dangerous direction only:
//! overcounting is safe and stays visible in the size baseline, not here.
//! ISR rows carry `has_isr`: the prologue saves all 4 retval bytes with
//! no IR shape to scan, so the report forces the full count, and the
//! assertion prices the ISR base (12/9) plus the 4 the same way.

use std::path::PathBuf;
use std::process::Command;

struct Case {
    name: &'static str,
    device: &'static str,
    fixture: &'static str,
    has_isr: bool,
}

fn cases() -> Vec<Case> {
    let pic18 = [
        ("add", "add.c"),
        ("bool", "size-bench/bench-bool.c"),
        ("dead-store", "size-bench/bench-dead-store.c"),
        ("switch", "size-bench/bench-switch.c"),
        ("branch-computed", "size-bench/bench-branch-computed.c"),
        ("w-roundtrip", "size-bench/bench-w-roundtrip.c"),
        ("const-sub", "size-bench/bench-const-sub.c"),
        ("u16-dec", "size-bench/bench-u16-dec.c"),
        ("u32-loop", "size-bench/bench-u32-loop.c"),
        ("shift", "size-bench/bench-shift.c"),
        ("struct-copy", "size-bench/bench-struct-copy.c"),
        ("delay", "delay.c"),
    ];
    let pic14 = [
        ("add", "add.c"),
        ("struct-scan", "size-bench/bench-struct-scan.c"),
        ("delay", "delay.c"),
        ("const-3chunk", "const_3chunk.c"),
        ("dynamic-memcpy", "dynamic_memcpy.c"),
        ("pid-clamp", "pid_clamp.c"),
    ];
    let mut out: Vec<Case> = pic18
        .iter()
        .map(|(n, f)| Case {
            name: n,
            device: "18F4550",
            fixture: f,
            has_isr: false,
        })
        .collect();
    out.extend(pic14.iter().map(|(n, f)| Case {
        name: n,
        device: "16F877A",
        fixture: f,
        has_isr: false,
    }));
    out.push(Case {
        name: "add",
        device: "16F1937",
        fixture: "add.c",
        has_isr: false,
    });
    // ISR rows: the prologue's 4-byte retval save has no IR shape, so
    // these are the rows that fail if the `has_isr` forcing regresses.
    out.push(Case {
        name: "fsr1-isr",
        device: "18F4550",
        fixture: "fsr1_isr.c",
        has_isr: true,
    });
    out.push(Case {
        name: "isr-ticks",
        device: "16F877A",
        fixture: "volatile_isr_ticks.c",
        has_isr: true,
    });
    out
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn compile(c: &Case, emit_asm: bool) -> (String, String) {
    let out = std::env::temp_dir().join(format!(
        "fixed-uses-{}-{}-{}.{}",
        c.device,
        c.name,
        std::process::id(),
        if emit_asm { "asm" } else { "hex" }
    ));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args(["--target", c.device]);
    if emit_asm {
        cmd.args(["--emit", "asm"]);
    }
    cmd.arg("-o").arg(&out).arg(fixtures_dir().join(c.fixture));
    let proc = cmd.output().expect("run epic-cc");
    assert!(
        proc.status.success(),
        "epic-cc {} ({}): {}",
        c.name,
        c.device,
        String::from_utf8_lossy(&proc.stderr)
    );
    let text = std::fs::read_to_string(&out).expect("read compiler output");
    let _ = std::fs::remove_file(&out);
    (String::from_utf8_lossy(&proc.stderr).into_owned(), text)
}

/// Reported `fixed:`/`common:` bytes from the size report.
fn reported_fixed(report: &str) -> u16 {
    let line = report
        .lines()
        .find(|l| l.contains("fixed:") || l.contains("common:"))
        .expect("size report has a fixed/common line");
    let after = line.split(':').nth(1).expect("fixed line has a colon");
    after
        .split('/')
        .next()
        .expect("fixed line has a total")
        .trim()
        .parse()
        .expect("fixed count parses")
}

/// Highest PIC18 fixed byte the asm references: `0x000`..`0x003` plus the
/// `0x0000` flag bit (byte 0 either way). The `org` line is not a touch,
/// and neither are W-literal operands (`RETLW` table data, `MOVLW` counts)
/// or `equ` aliases.
fn touched_pic18(asm: &str) -> u16 {
    let mut hi: Option<u8> = None;
    let mut flag = false;
    for line in asm.lines() {
        if line.trim_start().starts_with("org") || line.contains("equ") || line.contains("LW ") {
            continue;
        }
        for b in 0..4u8 {
            if line.contains(&format!("0x{b:03X}")) {
                hi = Some(hi.map_or(b, |h| h.max(b)));
            }
        }
        if line.contains("0x0000") {
            flag = true;
        }
    }
    hi.map_or(0, |h| u16::from(h) + 1).max(u16::from(flag))
}

/// Highest PIC14/PIC14E retval byte the asm references, as a count:
/// `0x71`..`0x74`. The 0x70 scratch is always counted, never scanned.
fn touched_pic14(asm: &str) -> u16 {
    let mut hi = 0u8;
    for line in asm.lines() {
        if line.contains("LW ") || line.contains("equ") {
            continue;
        }
        for b in 0x71..=0x74u8 {
            if line.contains(&format!("0x{b:02X}")) {
                hi = hi.max(b);
            }
        }
    }
    if hi == 0 {
        0
    } else {
        u16::from(hi - 0x71 + 1)
    }
}

#[test]
fn fixed_report_covers_emitted_touches() {
    for c in cases() {
        let (report, _) = compile(&c, false);
        let (_, asm) = compile(&c, true);
        let reported = reported_fixed(&report);
        // The ISR base rides along untouched by the scan: price it from
        // the case, so an ISR row fails if the `has_isr` forcing drops
        // the retval term the prologue always touches.
        let isr_base = match (c.device, c.has_isr) {
            (_, false) => 0,
            ("18F4550", true) => 12,
            _ => 9,
        };
        let (touched, floor) = match c.device {
            "18F4550" => (touched_pic18(&asm), 0),
            _ => (touched_pic14(&asm), 1),
        };
        println!(
            "{}-{}: reported fixed {reported}, touched retval {touched}",
            c.device, c.name
        );
        assert!(
            reported >= floor + isr_base + touched,
            "{} ({}): reported fixed {reported} below touched {touched}:\n{asm}",
            c.name,
            c.device
        );
    }
}
