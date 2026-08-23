//! Per-part memory-map and capability facts, threaded through `alloc`,
//! `isel`, `banking`, and `driver` instead of being hard-coded PIC16F877A
//! literals in each of them. See docs/29-pic18-port-design.md (§2 D-3) for
//! the design this implements. P1 adds the PIC18F4550 profile;
//! `has_hardware_multiply`/`has_tblrd`/`sfrs` still aren't added (unused so
//! far) and `access_bank` never will be  -  it's a core PIC18 invariant, not
//! a per-device fact. P2 populates `PIC18F4550`'s `ram_banks`/`common_ram` for real (P0/P1 left them as placeholders since nothing consumed them yet).

mod config;
pub use config::resolve_config;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Core {
    Pic14,
    Pic18,
    Pic14e,
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
    pub config: ConfigRegion,
}
include!(concat!(env!("OUT_DIR"), "/devices.rs"));

impl Device {
    /// The first GPR bank's start address  -  where global allocation begins.
    pub fn gpr_start(&self) -> u16 {
        self.ram_banks[0].0
    }

    /// The inclusive `(start, end)` of the banked GPR region containing
    /// `addr`  -  the first bank (in address order) whose end is `>= addr`,
    /// so an `addr` before the first bank's start still resolves to that
    /// bank (matching every caller, which only ever asks with `addr` at or
    /// above `gpr_start()`). `None` once `addr` is past the last bank.
    pub fn region_for(&self, addr: u16) -> Option<(u16, u16)> {
        self.ram_banks
            .iter()
            .find(|&&(_, end)| addr <= end)
            .copied()
    }

    /// The bank index of a physical GPR address: `Some(n)` for a banked GPR
    /// address, `None` for an SFR or common-RAM address (neither needs a
    /// `BANKSEL`). Panics for an address in neither category (an
    /// unimplemented/reserved gap)  -  such an address must never reach the
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
        if addr == 0x80 || addr == 0x180 {
            panic!(
                "device: 0x{addr:03X} is not a banked GPR address on {}",
                self.name
            );
        }
        None
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FuseValue {
    pub name: &'static str,
    /// The raw bit pattern this value encodes, already positioned at bit 0
    /// (shifted into place by `resolve_config` using the field's `shift`).
    pub bits: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct FuseField {
    pub name: &'static str,
    /// Offset into the region, 0-based (e.g. CONFIG4L is offset 6 in the
    /// PIC18F4550's region, which starts at `base_byte_addr`).
    pub byte_offset: u16,
    pub mask: u8,
    pub shift: u8,
    pub values: &'static [FuseValue],
    /// `None`: no safe default exists (oscillator-tree fields); an
    /// EPIC_CONFIG override is required, or resolution panics.
    pub default: Option<&'static str>,
    /// `Some(name)`: the only value epic-cc's backend can honor. An
    /// override to anything else panics. Used for `XINST` only.
    pub locked: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
pub struct ConfigRegion {
    pub base_byte_addr: u32,
    pub num_bytes: u16,
    /// Raw flash content before any FuseField is applied. Confirmed
    /// per-device, not assumed uniform; see docs/31 D-9.
    pub erased_baseline: &'static [u8],
    pub fields: &'static [FuseField],
}
