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

/// Allocatable data memory a generic `.lkr` declares, kept in three lists
/// rather than one total: `isel` derives `fsr_window` from where the banked
/// and bank-independent regions meet, so merging them hides a moved boundary.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LkrRam {
    /// `DATABANK` entries named `gpr*`, in address order.
    pub banks: Vec<(u16, u16)>,
    /// Unprotected `SHAREBANK` entries: the PIC14 common window.
    pub shared: Vec<(u16, u16)>,
    /// Unprotected `ACCESSBANK` entries: the PIC18 access RAM.
    pub access: Vec<(u16, u16)>,
}

enum Kind {
    Bank,
    Shared,
    Access,
}

/// Reads the allocatable regions of a generic `.lkr`. `PROTECTED` marks SFR
/// banks and the common-RAM mirrors, which are not allocatable.
///
/// gplink's `#IFDEF` guards are evaluated with **no symbol defined**, which is
/// how epic-cc assembles: it links nothing from gputils, so `_CRUNTIME`,
/// `_EXTENDEDMODE` and `_DEBUGSTACK` are all off. On the 4550 that selects the
/// `#ELSE` arm, `ACCESSBANK accessram`, over the extended-mode `DATABANK gpre`.
pub fn ram_from_lkr(text: &str) -> LkrRam {
    let mut out = LkrRam::default();
    // One bool per open guard: whether the arm being read is the live one.
    let mut arms: Vec<bool> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("#IFDEF") {
            arms.push(false);
            continue;
        }
        if line.starts_with("#IFNDEF") {
            arms.push(true);
            continue;
        }
        if line.starts_with("#ELSE") {
            if let Some(live) = arms.last_mut() {
                *live = !*live;
            }
            continue;
        }
        if line.starts_with("#FI") || line.starts_with("#ENDIF") {
            arms.pop();
            continue;
        }
        if arms.iter().any(|live| !live) {
            continue;
        }
        let (kind, rest) = if let Some(r) = line.strip_prefix("DATABANK") {
            (Kind::Bank, r)
        } else if let Some(r) = line.strip_prefix("SHAREBANK") {
            (Kind::Shared, r)
        } else if let Some(r) = line.strip_prefix("ACCESSBANK") {
            (Kind::Access, r)
        } else {
            continue;
        };
        if rest.contains("PROTECTED") {
            continue;
        }
        let field = |key: &str| -> Option<&str> {
            rest.split_whitespace().find_map(|t| t.strip_prefix(key))
        };
        if matches!(kind, Kind::Bank) && !field("NAME=").unwrap_or("").starts_with("gpr") {
            continue;
        }
        let hex = |s: &str| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok();
        if let (Some(lo), Some(hi)) = (field("START=").and_then(hex), field("END=").and_then(hex)) {
            match kind {
                Kind::Bank => out.banks.push((lo, hi)),
                Kind::Shared => out.shared.push((lo, hi)),
                Kind::Access => out.access.push((lo, hi)),
            }
        }
    }
    out.banks.sort_unstable();
    out.shared.sort_unstable();
    out.access.sort_unstable();
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
    fn separates_banked_gpr_from_the_shared_window() {
        let ram = ram_from_lkr(LKR);
        assert_eq!(ram.banks, vec![(0x20, 0x6F), (0xA0, 0xEF)]);
        assert_eq!(ram.shared, vec![(0x70, 0x7F)]);
        assert!(ram.access.is_empty());
    }

    #[test]
    fn skips_protected_lines_and_non_gpr_databanks() {
        let ram = ram_from_lkr(LKR);
        assert!(!ram.banks.contains(&(0x0, 0x1F)), "sfr0 must not count");
        assert!(
            !ram.shared.contains(&(0xF0, 0xFF)),
            "protected mirror must not count"
        );
    }

    const GUARDED: &str = "\
#IFDEF _EXTENDEDMODE
  DATABANK   NAME=gpre       START=0x0               END=0x5F
#ELSE
  ACCESSBANK NAME=accessram  START=0x0               END=0x5F
#FI

DATABANK   NAME=gpr0       START=0x60              END=0xFF
ACCESSBANK NAME=accesssfr  START=0xF60             END=0xFFF          PROTECTED
";

    #[test]
    fn takes_the_else_arm_because_no_symbol_is_defined() {
        let ram = ram_from_lkr(GUARDED);
        assert_eq!(ram.access, vec![(0x0, 0x5F)], "accessram is the live arm");
        assert_eq!(ram.banks, vec![(0x60, 0xFF)], "gpre is the dead arm");
    }
}
