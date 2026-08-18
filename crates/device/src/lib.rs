//! Per-part memory-map and capability facts, threaded through `alloc`,
//! `isel`, `banking`, and `driver` instead of being hard-coded PIC16F877A
//! literals in each of them. See docs/29-pic18-port-design.md (§2 D-3) for
//! the design this implements. P1 adds the PIC18F4550 profile;
//! `has_hardware_multiply`/`has_tblrd`/`sfrs` still aren't added (unused so
//! far) and `access_bank` never will be — it's a core PIC18 invariant, not
//! a per-device fact (see docs/superpowers/plans/2026-08-18-pic18-port-p1.md).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Core {
    Pic14,
    Pic18,
}

#[derive(Clone, Copy, Debug)]
pub struct Device {
    pub name: &'static str,
    pub core: Core,
    /// Total flash size in 14-bit words.
    pub flash_words: u32,
    /// Every banked GPR region, in address order: inclusive `(start, end)`.
    pub ram_banks: &'static [(u16, u16)],
    /// The bank-independent common-RAM range, inclusive, if the core has one
    /// (`None` for a PIC18 Access-Bank device).
    pub common_ram: Option<(u16, u16)>,
    /// Hardware call-stack depth; recursion beyond it is rejected at legalize.
    pub stack_depth: u8,
    /// Interrupt vector word address(es): one for PIC14; two (high/low
    /// priority) for a PIC18 device with IPEN set.
    pub interrupt_vectors: &'static [u16],
}

pub const PIC16F877A: Device = Device {
    name: "p16f877a",
    core: Core::Pic14,
    flash_words: 0x2000,
    ram_banks: &[(0x20, 0x6F), (0xA0, 0xEF), (0x120, 0x16F), (0x1A0, 0x1EF)],
    common_ram: Some((0x70, 0x7F)),
    stack_depth: 8,
    interrupt_vectors: &[0x0004],
};

/// The PIC18F2455/2550/4455/4550 family (the 4550 profile specifically —
/// the others share the core with less flash/RAM). `ram_banks`/`common_ram`
/// are the PIC14 banked-GPR-region model, which PIC18's Access Bank + BSR
/// scheme doesn't use — left empty/`None` here since nothing consumes them
/// for a Pic18 device yet (P1 has no allocator; the access-bank/BSR
/// addressing model itself is a core PIC18 architecture invariant, not a
/// per-device fact, so it isn't a `Device` field at all — see
/// docs/superpowers/plans/2026-08-18-pic18-port-p1.md).
pub const PIC18F4550: Device = Device {
    name: "p18f4550",
    core: Core::Pic18,
    flash_words: 0x4000,
    ram_banks: &[],
    common_ram: None,
    stack_depth: 31,
    interrupt_vectors: &[0x0008, 0x0018],
};

impl Device {
    /// The first GPR bank's start address — where global allocation begins.
    pub fn gpr_start(&self) -> u16 {
        self.ram_banks[0].0
    }

    /// The inclusive `(start, end)` of the banked GPR region containing
    /// `addr` — the first bank (in address order) whose end is `>= addr`,
    /// so an `addr` before the first bank's start still resolves to that
    /// bank (matching every caller, which only ever asks with `addr` at or
    /// above `gpr_start()`). `None` once `addr` is past the last bank.
    pub fn region_for(&self, addr: u16) -> Option<(u16, u16)> {
        self.ram_banks.iter().find(|&&(_, end)| addr <= end).copied()
    }

    /// The bank index of a physical GPR address: `Some(n)` for a banked GPR
    /// address, `None` for an SFR or common-RAM address (neither needs a
    /// `BANKSEL`). Panics for an address in neither category (an
    /// unimplemented/reserved gap) — such an address must never reach the
    /// banking pass.
    pub fn bank_of(&self, addr: u16) -> Option<u8> {
        if let Some((lo, hi)) = self.common_ram {
            if addr >= lo && addr <= hi {
                return None;
            }
        }
        for (i, &(start, end)) in self.ram_banks.iter().enumerate() {
            if addr >= start && addr <= end {
                return Some(i as u8);
            }
        }
        if addr < self.gpr_start() {
            return None; // SFR range, below the first GPR bank
        }
        panic!("device: 0x{addr:03X} is not a banked GPR address on {}", self.name);
    }
}
