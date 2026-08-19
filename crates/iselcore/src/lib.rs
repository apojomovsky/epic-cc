//! `iselcore` — shared instruction-selection primitives used by both the
//! PIC14 (`isel`) and PIC18 (`isel-pic18`) backends.

use std::collections::HashMap;

/// Map key for a local value: `{func}::{name}` (IR value names without `%`).
/// Matches the keys `alloc` emits in its overlay layout, so a callee's param
/// slots and the caller's live slots never collide across CALL boundaries.
pub fn ssa_key(func: &str, name: &str) -> String {
    format!("{func}::{name}")
}

/// Where a local's bytes live. v1 only ever constructs `Direct` — introduced
/// now (docs/29-pic18-port-design.md §2 D-2) so a later frame-pointer phase
/// (recursion/reentrancy) never has to touch the call sites that resolve a
/// local's address, only add a real `Frame` case here and wherever
/// `Slot` values get constructed.
pub enum Slot {
    /// Statically allocated: a direct file address.
    Direct(u16),
    /// Frame-relative, FSR2 + offset. Reserved for the later reentrancy
    /// phase; nothing constructs this yet.
    #[allow(dead_code)]
    Frame(i8),
}

impl Slot {
    /// v1 only ever constructs `Direct`.
    pub fn direct(&self) -> u16 {
        match self {
            Slot::Direct(a) => *a,
            Slot::Frame(_) => unimplemented!("frame-relative slots arrive with the reentrancy phase"),
        }
    }
}

/// Parse an alloc-produced address-map text into `HashMap<String, u16>`:
/// `global <name> 0xNN` and `local <func> <name> 0xNN` lines become map
/// entries (locals keyed `{func}::{name}`); `const <name>` lines list flash
/// globals, which have no RAM address, so they are accepted and skipped —
/// isel reads their bytes from the `Module`, never from a RAM slot.
///
/// Shared between both backends (moved here from `isel`, which only ever
/// had this because `isel-pic18` pulled it in via a hard `isel` dependency
/// that existed for no other reason — see the plan's final-review fix
/// notes). Nothing about this parser is PIC14-specific; it is a plain
/// text-format parser over `alloc`'s output.
pub fn parse_map(text: &str) -> HashMap<String, u16> {
    let mut addrs = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        let mut it = line.split_whitespace();
        let kw = it.next().expect("map entry");
        match kw {
            "const" => {
                // Flash global: no RAM address; nothing to record.
            }
            "global" => {
                let name = it
                    .next()
                    .unwrap_or_else(|| panic!("iselcore: malformed map line: {line}"))
                    .to_string();
                let addr = it
                    .next()
                    .and_then(|h| u16::from_str_radix(h.trim_start_matches("0x"), 16).ok())
                    .unwrap_or_else(|| panic!("iselcore: bad address in map line: {line}"));
                addrs.insert(name, addr);
            }
            "local" => {
                let func = it
                    .next()
                    .unwrap_or_else(|| panic!("iselcore: malformed map line: {line}"))
                    .to_string();
                let name = it
                    .next()
                    .unwrap_or_else(|| panic!("iselcore: malformed map line: {line}"))
                    .to_string();
                let addr = it
                    .next()
                    .and_then(|h| u16::from_str_radix(h.trim_start_matches("0x"), 16).ok())
                    .unwrap_or_else(|| panic!("iselcore: bad address in map line: {line}"));
                addrs.insert(format!("{func}::{name}"), addr);
            }
            _ => panic!("iselcore: unexpected map line: {line}"),
        }
    }
    addrs
}
