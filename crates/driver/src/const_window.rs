//! Post-peephole const-table window repair (epic-cc#879).
//!
//! isel folds each small table's base inside one 256-byte window, but it
//! measures the section start before the driver's `schedule` pass runs, and
//! schedule's reordering changes the BANKSEL count banking inserts. The
//! section then starts a few words off the model: shrinkage drags tables
//! back across window starts, growth pushes them across window ends, and
//! the assembler's `.table` assert panics on the crossing.
//!
//! This pass walks the final text with the assembler's own pass-1
//! addresses and inserts `.align 256` ahead of any table that would cross.
//! One forward pass is exact: pads only move later content forward, and
//! every later table is checked at its padded address. Fitting layouts
//! pass through byte-identical, so the size ladder never moves.

use ir::SrcLoc;
use iselcore::is_directive;

/// The repaired text plus a parallel per-line source-location vector, kept
/// index-aligned with the returned text: an inserted `.align` carries
/// `None` like every other compiler-generated line. Runs on the PIC14 and
/// PIC14E arms alike, whose `.table`/`.align` shapes are identical.
pub fn repair_with_locs(asm: &str, locs: &[Option<SrcLoc>]) -> (String, Vec<Option<SrcLoc>>) {
    let lines: Vec<&str> = asm.lines().collect();
    assert!(
        locs.len() == lines.len(),
        "const-window repair needs one loc per line, got {} locs for {} lines",
        locs.len(),
        lines.len()
    );
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut out_locs: Vec<Option<SrcLoc>> = Vec::with_capacity(lines.len());
    let mut org = 0usize;
    // A reader extent (`__read_*` label to its table) open across the walk,
    // with the word address its label sits at. Only extents placed after
    // the first inserted pad are page-checked: earlier ones are isel's own
    // pinned layout, which this pass must not second-guess.
    let mut reader: Option<(String, usize)> = None;
    let mut padded = false;
    let mut ended = false;
    let close_reader = |reader: &mut Option<(String, usize)>, end: usize, check: bool| {
        if let Some((name, start)) = reader.take() {
            if check && end > start {
                let last = end - 1;
                assert!(
                    start / 0x800 == last / 0x800,
                    "const-window repair moved const reader {name} across a page boundary (0x{start:04X}-0x{last:04X}): re-pin it to one page"
                );
            }
        }
    };
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if ended || line.is_empty() || is_directive(line, "list") || is_directive(line, "radix") {
            out.push(raw.to_string());
            out_locs.push(locs[i].clone());
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            close_reader(&mut reader, org, padded);
            let target = parse_num(rest.trim());
            assert!(
                target >= org,
                "const-window repair hit a backward .org to 0x{target:04X} from 0x{org:04X}: pads only move forward, so this layout is unrepairable"
            );
            org = target;
            out.push(raw.to_string());
            out_locs.push(locs[i].clone());
            continue;
        }
        if is_directive(line, "end") {
            close_reader(&mut reader, org, padded);
            ended = true;
            out.push(raw.to_string());
            out_locs.push(locs[i].clone());
            continue;
        }
        if let Some(colon) = line.find(':') {
            let label = line[..colon].trim();
            let rest = line[colon + 1..].trim();
            if !label.is_empty() && !label.contains(' ') && !label.contains('\t') {
                close_reader(&mut reader, org, padded);
                if label.starts_with("__read_") {
                    reader = Some((label.to_string(), org));
                }
                out.push(raw.to_string());
                out_locs.push(locs[i].clone());
                if !rest.is_empty() {
                    org += 1;
                }
                continue;
            }
        }
        if line.contains(" equ ") {
            out.push(raw.to_string());
            out_locs.push(locs[i].clone());
            continue;
        }
        if let Some(n) = line.strip_prefix(".align ") {
            org = align_up(org, parse_num(n.trim()));
            out.push(raw.to_string());
            out_locs.push(locs[i].clone());
            continue;
        }
        if let Some(rest) = line.strip_prefix(".table ") {
            close_reader(&mut reader, org, padded);
            let mut it = rest.split_whitespace();
            let name = it.next().unwrap_or("?");
            let size = parse_num(it.next().unwrap_or("0"));
            let base = if size <= 255 {
                if (org & 0xFF) + size <= 0x100 {
                    org
                } else {
                    align_up(org, 256)
                }
            } else if org & 0xFF == 0 {
                org
            } else {
                align_up(org, 256)
            };
            if base != org {
                assert!(
                    base / 0x800 == org / 0x800,
                    "const-window repair would move const table {name} of {size} bytes across a page boundary (0x{org:04X} to 0x{base:04X}): the reader-page restore map cannot follow"
                );
                out.push("    .align 256".to_string());
                out_locs.push(None);
                org = base;
                padded = true;
            }
            out.push(raw.to_string());
            out_locs.push(locs[i].clone());
            continue;
        }
        out.push(raw.to_string());
        out_locs.push(locs[i].clone());
        org += 1;
    }
    close_reader(&mut reader, org, padded);
    let mut text = out.join("\n");
    if asm.ends_with('\n') {
        text.push('\n');
    }
    (text, out_locs)
}

/// Round `org` up to the next `n`-word boundary, the assembler's `.align`
/// semantics for the power-of-two sizes isel emits.
fn align_up(org: usize, n: usize) -> usize {
    (org + n - 1) & !(n - 1)
}

/// A decimal or `0x`-prefixed hex number, the assembler's literal grammar.
fn parse_num(s: &str) -> usize {
    if let Some(h) = s.strip_prefix("0x") {
        usize::from_str_radix(h, 16).unwrap()
    } else {
        s.parse().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locs_for(asm: &str) -> Vec<Option<SrcLoc>> {
        vec![None; asm.lines().count()]
    }

    /// Header `equ`s isel always emits, so the test texts assemble as is.
    const HDR: &str = "PCL    equ 0x02\nPCLATH equ 0x0A\n";

    /// One table reader plus its table, the emitter's shape: a 6-word
    /// entry, the `.table` directive, the base label, then the bytes.
    fn table_unit(tag: &str, decl_size: usize, bytes: &[u8]) -> String {
        let mut s = format!(
            "__read_{tag}:\n    MOVWF 0x70\n    MOVLW HIGH({tag})\n    MOVWF PCLATH\n    MOVF 0x70, W\n    ADDLW LOW({tag})\n    MOVWF PCL\n    .table {tag} {decl_size}\n{tag}:\n"
        );
        for b in bytes {
            s.push_str(&format!("    RETLW 0x{b:02X}\n"));
        }
        s
    }

    fn filler(n: usize) -> String {
        (0..n).map(|_| "    MOVLW 0x00\n").collect()
    }

    #[test]
    fn crossing_table_gets_an_align_pad() {
        // Simulated drift: 6 reader words ahead of a base at LOW 0xEA, so
        // the 35-byte table crosses its window. The repair pads it to the
        // next boundary, the emitter's own shape, and the bytes land.
        let asm =
            format!("{}    org 0x0000\n{}", HDR, filler(0xEA)) + &table_unit("t", 35, &[1; 35]);
        let (fixed, fixed_locs) = repair_with_locs(&asm, &locs_for(&asm));
        assert!(
            fixed.contains("    .align 256\n    .table t 35"),
            "pad:\n{fixed}"
        );
        assert_eq!(fixed_locs.len(), fixed.lines().count());
        let words = asm::assemble(&fixed);
        let base = words
            .iter()
            .position(|w| *w == (0x3400 | 1))
            .expect("table bytes land");
        assert_eq!(base & 0xFF, 0, "padded base is window-aligned");
        assert!(words[base..base + 35].iter().all(|w| *w == (0x3400 | 1)));
    }

    #[test]
    fn exact_fit_is_untouched() {
        // LOW + size == 0x100 fits by the assembler's own rule, so the
        // repair must leave the text byte-identical: no ladder row moves.
        let asm =
            format!("{}    org 0x0000\n{}", HDR, filler(0xD7)) + &table_unit("t", 35, &[2; 35]);
        assert_eq!(((0xD7 + 6) & 0xFF) + 35, 0x100, "fixture fits exactly");
        let (fixed, _) = repair_with_locs(&asm, &locs_for(&asm));
        assert_eq!(fixed, asm, "no-op");
    }

    #[test]
    fn second_table_cascade_gets_its_own_pad() {
        // The first pad shifts the second table across its window end; the
        // single forward pass checks it at the padded address, not the old
        // one, so both land aligned.
        let asm = format!("{}    org 0x0000\n{}", HDR, filler(0xEA))
            + &table_unit("t", 64, &[1; 64])
            + &table_unit("u", 200, &[4; 35]);
        let (fixed, _) = repair_with_locs(&asm, &locs_for(&asm));
        assert_eq!(
            fixed.matches("    .align 256").count(),
            2,
            "one pad per table:\n{fixed}"
        );
        asm::assemble(&fixed);
    }

    #[test]
    fn chunked_table_off_boundary_gets_padded() {
        // A 300-byte table must sit at LOW 0; drift left it at LOW 0xFC.
        let mut asm =
            format!("{}    org 0x0000\n{}", HDR, filler(0xF6)) + &table_unit("big", 300, &[3; 256]);
        asm.push_str("big_1:\n");
        for _ in 0..44 {
            asm.push_str("    RETLW 0x03\n");
        }
        let (fixed, _) = repair_with_locs(&asm, &locs_for(&asm));
        assert!(
            fixed.contains("    .align 256\n    .table big 300"),
            "pad:\n{fixed}"
        );
        asm::assemble(&fixed);
    }

    #[test]
    #[should_panic(expected = "across a page boundary")]
    fn pad_refuses_a_page_crossing() {
        // The pad would carry the base into the next page, where the
        // reader-page restore map no longer holds: fail loudly instead.
        let asm =
            format!("{}    org 0x0000\n{}", HDR, filler(0x7EA)) + &table_unit("t", 35, &[1; 35]);
        let _ = repair_with_locs(&asm, &locs_for(&asm));
    }

    #[test]
    #[should_panic(expected = "backward .org")]
    fn backward_org_fails_loudly() {
        let asm = "    org 0x0100\n    MOVLW 0x00\n    org 0x00F0\n    end\n";
        let _ = repair_with_locs(asm, &locs_for(asm));
    }
}
