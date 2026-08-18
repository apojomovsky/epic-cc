//! `iselcore` — shared instruction-selection primitives used by both the
//! PIC14 (`isel`) and PIC18 (`isel-pic18`) backends.

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
