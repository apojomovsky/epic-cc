//! Parsing of the gputils generic linker script, used only to cross-check the
//! committed device data. Nothing here is copied into a device TOML.

/// Merges overlapping and adjacent ranges into maximal disjoint ones.
/// Adjacency matters: gputils splits a flat PIC18 window into nine banks that
/// describe the same memory as one TOML entry.
pub fn coalesce(ranges: &[(u16, u16)]) -> Vec<(u16, u16)> {
    let mut sorted = ranges.to_vec();
    sorted.sort_unstable();
    let mut out: Vec<(u16, u16)> = Vec::new();
    for (lo, hi) in sorted {
        match out.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}

/// General-purpose RAM described by a generic `.lkr`: `DATABANK` entries named
/// `gpr*` plus unprotected `SHAREBANK`s. `PROTECTED` marks SFR banks and the
/// common-RAM mirrors, which are not allocatable.
pub fn gpr_ranges_from_lkr(text: &str) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let kind = if let Some(r) = line.strip_prefix("DATABANK") {
            ("DATABANK", r)
        } else if let Some(r) = line.strip_prefix("SHAREBANK") {
            ("SHAREBANK", r)
        } else {
            continue;
        };
        if kind.1.contains("PROTECTED") {
            continue;
        }
        let field = |key: &str| -> Option<&str> {
            kind.1.split_whitespace().find_map(|t| t.strip_prefix(key))
        };
        let name = field("NAME=").unwrap_or("");
        if kind.0 == "DATABANK" && !name.starts_with("gpr") {
            continue;
        }
        let hex = |s: &str| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok();
        if let (Some(lo), Some(hi)) = (field("START=").and_then(hex), field("END=").and_then(hex)) {
            out.push((lo, hi));
        }
    }
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesce_merges_adjacent_and_overlapping_ranges() {
        assert_eq!(coalesce(&[(0x20, 0x6F), (0x70, 0x7F)]), vec![(0x20, 0x7F)]);
        assert_eq!(coalesce(&[(0x20, 0x40), (0x30, 0x7F)]), vec![(0x20, 0x7F)]);
        assert_eq!(
            coalesce(&[(0xA0, 0xEF), (0x20, 0x6F)]),
            vec![(0x20, 0x6F), (0xA0, 0xEF)]
        );
    }

    const LKR: &str = "\
DATABANK   NAME=sfr0       START=0x0               END=0x1F           PROTECTED
DATABANK   NAME=gpr0       START=0x20              END=0x6F
DATABANK   NAME=gpr1       START=0xA0              END=0xEF
SHAREBANK  NAME=gprnobnk   START=0x70            END=0x7F
SHAREBANK  NAME=gprnobnk   START=0xF0            END=0xFF           PROTECTED
CODEPAGE   NAME=page0      START=0x0               END=0x7FF
";

    #[test]
    fn parses_gpr_databanks_and_unprotected_sharebanks() {
        assert_eq!(
            gpr_ranges_from_lkr(LKR),
            vec![(0x20, 0x6F), (0x70, 0x7F), (0xA0, 0xEF)]
        );
    }

    #[test]
    fn skips_protected_lines_and_non_gpr_databanks() {
        let got = gpr_ranges_from_lkr(LKR);
        assert!(!got.contains(&(0x0, 0x1F)), "sfr0 must not count");
        assert!(
            !got.contains(&(0xF0, 0xFF)),
            "protected mirror must not count"
        );
    }
}
