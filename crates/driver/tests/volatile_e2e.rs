//! Volatile-access regression pins (epic-cc#812).
//!
//! `irparse` drops the `volatile` marker and `ir` carries no bit, so no
//! pass past the boundary can tell a volatile access from an ordinary
//! one. Nothing elides or reorders them today only because every pass
//! treats all memory accesses conservatively. These tests pin that
//! property at the whole-pipeline level on both cores: drop, merge,
//! reorder, or hoist any access below and its probe fails.
//!
//! Each probe compiles its fixture twice (HEX for the sim, asm text for
//! access order/count) and asserts both surfaces.

use std::path::Path;
use std::process::Command;

struct Build {
    hex: String,
    map: String,
    asm: String,
}

fn run_driver(fixture: &str, device: &str, emit: &str, out: &Path, extra: &[&str]) {
    let mut argv = vec![
        fixture,
        "--device",
        device,
        "--emit",
        emit,
        "-o",
        out.to_str().unwrap(),
    ];
    argv.extend_from_slice(extra);
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(&argv)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver {fixture} {device} --emit {emit}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn build(fixture: &str, device: &str, stem: &str) -> Build {
    let dir = std::env::temp_dir().join(format!("volatile_{stem}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hex_path = dir.join("p.hex");
    let map_path = dir.join("p.map");
    let asm_path = dir.join("p.asm");
    run_driver(
        fixture,
        device,
        "hex",
        &hex_path,
        &["--map", map_path.to_str().unwrap()],
    );
    run_driver(fixture, device, "asm", &asm_path, &[]);
    Build {
        hex: std::fs::read_to_string(&hex_path).unwrap(),
        map: std::fs::read_to_string(&map_path).unwrap(),
        asm: std::fs::read_to_string(&asm_path).unwrap(),
    }
}

fn addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn pos(asm: &str, pat: &str, what: &str) -> usize {
    asm.find(pat)
        .unwrap_or_else(|| panic!("{what} ({pat}) not found:\n{asm}"))
}

/// Back-to-back volatile writes (the EECON2 0x55/0xAA shape): all four
/// stores land, in source order, on a global and on an SFR.
#[test]
fn volatile_double_write_keeps_all_four_stores_in_order() {
    for (dev, sfr_pat) in [("p16f877a", "MOVWF 0x06"), ("p18f4550", "MOVWF 0x081,A")] {
        let b = build(
            "tests/fixtures/volatile_double_write.c",
            dev,
            &format!("a_{dev}"),
        );
        let ee = addr(&b.map, "ee");
        let w = if dev.starts_with("p16") {
            format!("MOVWF 0x{ee:02X}")
        } else {
            format!("MOVWF 0x{ee:03X},A")
        };
        assert_eq!(
            b.asm.matches(w.as_str()).count(),
            2,
            "both writes to ee must emit ({w}) on {dev}:\n{}",
            b.asm
        );
        assert_eq!(
            b.asm.matches(sfr_pat).count(),
            2,
            "both SFR writes must emit ({sfr_pat}) on {dev}:\n{}",
            b.asm
        );
        let mut cursor = 0usize;
        for (i, pat) in [w.as_str(), w.as_str(), sfr_pat, sfr_pat]
            .iter()
            .enumerate()
        {
            let at = b.asm[cursor..]
                .find(pat)
                .unwrap_or_else(|| panic!("store {i} ({pat}) missing on {dev}:\n{}", b.asm))
                + cursor;
            cursor = at + pat.len();
        }
        let sfr_ram = if dev.starts_with("p16") { 0x06 } else { 0xF81 };
        if dev.starts_with("p16") {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&b.hex));
            p.run(200_000);
            assert!(p.halted(), "halted on {dev}");
            assert_eq!(p.ram()[ee], 0xAA, "ee final on {dev}");
            assert_eq!(p.ram()[sfr_ram], 0xAA, "SFR final on {dev}");
        } else {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&b.hex));
            p.run(200_000);
            assert!(p.halted(), "halted on {dev}");
            assert_eq!(p.ram()[ee], 0xAA, "ee final on {dev}");
            assert_eq!(p.ram()[sfr_ram], 0xAA, "SFR final on {dev}");
        }
    }
}

/// Read-after-write: the volatile load re-reads memory instead of
/// forwarding the stored value.
#[test]
fn volatile_read_after_write_reloads_from_memory() {
    for dev in ["p16f877a", "p18f4550"] {
        let b = build(
            "tests/fixtures/volatile_read_after_write.c",
            dev,
            &format!("b_{dev}"),
        );
        let shadow = addr(&b.map, "port_shadow");
        let flag = addr(&b.map, "flag");
        if dev.starts_with("p16") {
            let s = pos(&b.asm, &format!("MOVWF 0x{shadow:02X}"), "shadow store");
            let l = pos(&b.asm, &format!("MOVF 0x{shadow:02X}, W"), "shadow reload");
            let g = pos(&b.asm, &format!("MOVWF 0x{flag:02X}"), "flag store");
            assert!(
                s < l && l < g,
                "reload must sit between the stores:\n{}",
                b.asm
            );
        } else {
            let s = pos(&b.asm, &format!("MOVWF 0x{shadow:03X},A"), "shadow store");
            let l = pos(&b.asm, &format!("MOVFF 0x{shadow:03X}"), "shadow reload");
            assert!(s < l, "reload must follow the store:\n{}", b.asm);
        }
        if dev.starts_with("p16") {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&b.hex));
            p.run(200_000);
            assert!(p.halted(), "halted on {dev}");
            assert_eq!(p.ram()[shadow], 0x11, "shadow on {dev}");
            assert_eq!(p.ram()[flag], 0x11, "flag on {dev}");
        } else {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&b.hex));
            p.run(200_000);
            assert!(p.halted(), "halted on {dev}");
            assert_eq!(p.ram()[shadow], 0x11, "shadow on {dev}");
            assert_eq!(p.ram()[flag], 0x11, "flag on {dev}");
        }
    }
}

/// Polling loop: the flag load stays inside the loop, so an external
/// write to the flag exits it.
#[test]
fn volatile_poll_flag_reloads_inside_the_loop() {
    for dev in ["p16f877a", "p18f4550"] {
        let b = build(
            "tests/fixtures/volatile_poll_flag.c",
            dev,
            &format!("c_{dev}"),
        );
        let ready = addr(&b.map, "ready");
        let done = addr(&b.map, "done");
        let back = if dev.starts_with("p16") {
            "GOTO main_L1"
        } else {
            "BRA main_L1"
        };
        let load = if dev.starts_with("p16") {
            format!("MOVF 0x{ready:02X}, W")
        } else {
            format!("MOVF 0x{ready:03X}")
        };
        let top = pos(&b.asm, "main_L1:", "loop top");
        let tail = &b.asm[top..];
        let l = tail
            .find(&load)
            .map(|i| i + top)
            .unwrap_or_else(|| panic!("flag reload ({load}) not found:\n{}", b.asm));
        // The entry branch to the label precedes it, so the back edge
        // must be searched from the label on.
        let edge = tail
            .find(back)
            .map(|i| i + top)
            .unwrap_or_else(|| panic!("loop back edge ({back}) not found:\n{}", b.asm));
        assert!(l < edge, "reload must stay in the loop:\n{}", b.asm);
        if dev.starts_with("p16") {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&b.hex));
            p.run(50_000);
            assert!(!p.halted(), "still polling on {dev}");
            assert_eq!(p.ram()[done], 0, "done unset on {dev}");
            p.ram_mut()[ready] = 1;
            p.run(50_000);
            assert!(p.halted(), "exited on {dev}");
            assert_eq!(p.ram()[done], 1, "done set on {dev}");
        } else {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&b.hex));
            p.run(50_000);
            assert!(!p.halted(), "still polling on {dev}");
            assert_eq!(p.ram()[done], 0, "done unset on {dev}");
            p.ram_mut()[ready] = 1;
            p.run(50_000);
            assert!(p.halted(), "exited on {dev}");
            assert_eq!(p.ram()[done], 1, "done set on {dev}");
        }
    }
}

/// ISR-shared 16-bit counter: both bytes are re-read at each snapshot.
/// The sweep fires the ISR once at every step and requires a moved
/// snapshot, which only live reads produce.
#[test]
fn volatile_isr_ticks_rereads_both_bytes() {
    for dev in ["p16f877a", "p18f4550"] {
        let b = build(
            "tests/fixtures/volatile_isr_ticks.c",
            dev,
            &format!("d_{dev}"),
        );
        let ticks = addr(&b.map, "ticks");
        let lo = addr(&b.map, "lo");
        let hi = addr(&b.map, "hi");
        let is_pic14 = dev.starts_with("p16");
        if is_pic14 {
            assert!(
                b.asm.matches(&format!("MOVF 0x{ticks:02X}, W")).count() >= 2,
                "low byte read per snapshot on {dev}:\n{}",
                b.asm
            );
            assert!(
                b.asm
                    .matches(&format!("MOVF 0x{:02X}, W", ticks + 1))
                    .count()
                    >= 2,
                "high byte read per snapshot on {dev}:\n{}",
                b.asm
            );
        } else {
            assert!(
                b.asm.matches(&format!("MOVFF 0x{ticks:03X}")).count() >= 2,
                "low byte read per snapshot on {dev}:\n{}",
                b.asm
            );
            assert!(
                b.asm.matches(&format!("MOVFF 0x{:03X}", ticks + 1)).count() >= 2,
                "high byte read per snapshot on {dev}:\n{}",
                b.asm
            );
        }
        if is_pic14 {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&b.hex));
            p.run(200_000);
            assert!(p.halted(), "halted on {dev}");
            assert_eq!(
                (p.ram()[lo], p.ram()[hi]),
                (0x34, 0x12),
                "exact snapshot on {dev}"
            );
        } else {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&b.hex));
            p.run(200_000);
            assert!(p.halted(), "halted on {dev}");
            assert_eq!(
                (p.ram()[lo], p.ram()[hi]),
                (0x34, 0x12),
                "exact snapshot on {dev}"
            );
        }
        let mut ran = 0u32;
        let mut moved = 0u32;
        for k in 0..160usize {
            let (slo, shi, ok) = if is_pic14 {
                let mut q = pic14_sim::Pic14::new(pic14_sim::parse_hex(&b.hex));
                q.run(k);
                if q.halted() {
                    continue;
                }
                q.fire_interrupt();
                q.run(200_000);
                (q.ram()[lo], q.ram()[hi], q.halted())
            } else {
                let mut q = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&b.hex));
                q.run(k);
                if q.halted() {
                    continue;
                }
                q.fire_interrupt();
                q.run(200_000);
                (q.ram()[lo], q.ram()[hi], q.halted())
            };
            ran += 1;
            assert!(ok, "halted after fire at step {k} on {dev}");
            if slo != 0x34 || shi != 0x12 {
                moved += 1;
            }
        }
        assert!(ran > 20, "sweep covered the program on {dev} ({ran})");
        assert!(moved > 0, "a fire point moves the snapshot on {dev}");
    }
}

/// Cross-bank volatile order: differently-banked writes keep program
/// order through the PIC14 schedule pass, then the final re-read lands.
#[test]
fn volatile_bank_order_keeps_program_order() {
    for dev in ["p16f877a", "p18f4550"] {
        let b = build(
            "tests/fixtures/volatile_bank_order.c",
            dev,
            &format!("e_{dev}"),
        );
        let seq = addr(&b.map, "seq");
        if dev.starts_with("p16") {
            let first = pos(&b.asm, "MOVLW 0x01", "first value");
            let wf: Vec<usize> = b.asm.match_indices("MOVWF 0x05").map(|(i, _)| i).collect();
            assert_eq!(wf.len(), 3, "three banked SFR stores:\n{}", b.asm);
            let second_val = pos(&b.asm, "MOVLW 0x00", "second value");
            let third_val = pos(&b.asm, "MOVLW 0x02", "third value");
            assert!(
                first < wf[0]
                    && wf[0] < second_val
                    && second_val < wf[1]
                    && wf[1] < third_val
                    && third_val < wf[2],
                "writes in program order on {dev}:\n{}",
                b.asm
            );
            let load = pos(&b.asm, "MOVF 0x05, W", "final re-read");
            let seq_store = pos(&b.asm, &format!("MOVWF 0x{seq:02X}"), "seq store");
            assert!(
                wf[2] < load && load < seq_store,
                "re-read before seq:\n{}",
                b.asm
            );
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&b.hex));
            p.run(200_000);
            assert!(p.halted(), "halted on {dev}");
            assert_eq!(p.ram()[seq], 0x02, "seq on {dev}");
        } else {
            let w1 = pos(&b.asm, "MOVWF 0x081,A", "first SFR store");
            let mid = pos(&b.asm, "CLRF 0x082,A", "second SFR store");
            let w2 = b.asm[mid + 1..]
                .find("MOVWF 0x081,A")
                .map(|i| i + mid + 1)
                .unwrap_or_else(|| panic!("third store missing:\n{}", b.asm));
            assert!(
                w1 < mid && mid < w2,
                "writes in program order on {dev}:\n{}",
                b.asm
            );
            let load = pos(&b.asm, "MOVFF 0xF81", "final re-read");
            assert!(w2 < load, "re-read after the writes:\n{}", b.asm);
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&b.hex));
            p.run(200_000);
            assert!(p.halted(), "halted on {dev}");
            assert_eq!(p.ram()[seq], 0x02, "seq on {dev}");
        }
    }
}
