//! Per-part memory-map and capability facts, threaded through `alloc`,
//! `isel`, `banking`, and `driver` instead of being hard-coded PIC16F877A
//! literals in each of them. See docs/29 §2 for the design this implements.
//! `has_hardware_multiply`/`has_tblrd` aren't added (no consumer reads
//! them).

mod config;
pub use config::resolve_config;

pub mod gputils;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Core {
    Pic14,
    Pic18,
    Pic14e,
    PicBaseline,
}

#[derive(Clone, Copy, Debug)]
pub struct Device {
    pub name: &'static str,
    pub core: Core,
    /// Total flash size in 14-bit words.
    pub flash_words: u32,
    /// Every banked GPR region, in address order: inclusive `(start, end)`.
    pub ram_banks: &'static [(u16, u16)],
    /// The physically mirrored common-RAM window reachable from
    /// any bank with no `BANKSEL` (classic PIC14/PIC14E; on baseline the
    /// shared GPR window plays the same role; `None` on PIC18, where the
    /// access bank is modelled by `access_bank`).
    pub common_ram: Option<(u16, u16)>,
    /// PIC18 only: the hardware access bank, BSR-independent, as declared
    /// by gputils `ACCESSBANK` (`None` on PIC14, whose analogue is
    /// `common_ram`).
    pub access_bank: Option<(u16, u16)>,
    /// PIC18 only: compiler policy reservation inside the access bank for
    /// the fixed BSR-independent return-value region plus the ISR save
    /// spill area (`None` on PIC14). Not a hardware fact, so it is not
    /// cross-checked against the `.lkr`; it is where `isel-pic18` puts
    /// `retval_lo`.
    pub fixed_retval: Option<(u16, u16)>,
    /// PIC14/PIC14E only, and only on a device with `common_ram: None`
    /// (docs/39 D-2, e.g. PIC16F74's two non-aliased GPR regions): the
    /// fixed low-7-bit offset reserved out of *every* `ram_banks`
    /// region's normal allocation pool, reachable via direct addressing
    /// regardless of which region is currently selected (PIC14 direct
    /// addressing resolves `{RP1:RP0} ++ opcode<6:0>` at execution time,
    /// so one literal operand lands correctly in whichever region is
    /// live). The ISR prologue's only truly bank-independent byte: it
    /// stashes `W` here before it knows which region interrupted the
    /// main program. `None` on every device with `common_ram: Some` or a
    /// single `ram_banks` region, which have no need for it.
    pub isr_w_shadow: Option<u16>,
    /// Paired with `isr_w_shadow`: an ordinary (bank-selected, not
    /// bank-independent) window inside one `ram_banks` region that
    /// `isel` carves for the fixed scratch byte, 4-byte retval region,
    /// and ISR save area -- the same role `common_ram` plays on every
    /// other PIC14/PIC14E device, just reachable through `bank_of`'s
    /// normal `Some(bank)` path (so ordinary, non-ISR access still gets
    /// a real `BANKSEL` from `banking`, unlike true common RAM) instead
    /// of being exempt from banking entirely. `None` unless
    /// `isr_w_shadow` is also set.
    pub isr_home_window: Option<(u16, u16)>,
    /// Hardware call-stack depth; recursion beyond it is rejected at legalize.
    pub stack_depth: u8,
    /// Baseline only: how many of `FSR`'s high bits are bank select
    /// (`FSR<5>` on the p12f509, `FSR<6:5>` on the 16F505, unimplemented
    /// on the 508). Zero on every other core, whose bank selection never
    /// rides on `FSR`. Baseline bank changes are bit-granular
    /// `BCF`/`BSF FSR` sequences (docs/37 §2) and need this width.
    pub fsr_bank_bits: u8,
    /// Interrupt vector word address(es): one for PIC14; two (high/low
    /// priority) for a PIC18 device with IPEN set.
    pub interrupt_vectors: &'static [u16],
    pub config: ConfigRegion,
    /// The one artifact this registry shares with `epic-hal`, which generates
    /// its per family SFR headers from it. Empty for every device `cc` ships:
    /// the compiler needs the memory map and the config words, never a name.
    pub sfrs: &'static [Sfr],
}

#[derive(Clone, Copy, Debug)]
pub struct Sfr {
    pub name: &'static str,
    pub addr: u16,
    /// Width in bytes; 1 for every PIC14 and PIC18 SFR.
    pub width: u8,
    pub fields: &'static [SfrField],
}

#[derive(Clone, Copy, Debug)]
pub struct SfrField {
    pub name: &'static str,
    pub mask: u8,
    pub shift: u8,
}
/// The PIC14 core registers mirrored into every bank (epic-cc#112): only
/// these may be accessed with any RP1:RP0 value. Every other bank-0 SFR
/// (PORTA 0x05, TMR0 0x01, ...) exists solely in bank 0, so `bank_of`
/// returns `Some(0)` for it and the banking pass emits a BANKSEL. Mid-range
/// parts (877A, 887) share this map; TMR0 0x01 is deliberately absent
/// (OPTION_REG occupies its slot in banks 1/3).
const MIRRORED_SFRS: &[u16] = &[0x00, 0x02, 0x03, 0x04, 0x0A, 0x0B];

/// PIC14E's mirrored block, confirmed byte for byte against DS41364E's
/// memory map (Table 3-3): every bank's first 12 bytes are `INDF0, INDF1,
/// PCL, STATUS, FSR0L, FSR0H, FSR1L, FSR1H, BSR, WREG, PCLATH, INTCON`.
/// The design (docs/33 §1) names this as the encoder's `MIRRORED_SFRS` growth:
/// classic PIC14's six-register subset is wrong for the Enhanced core.
const MIRRORED_SFRS_PIC14E: &[u16] = &[
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
];

/// Baseline's mirrored block: the whole SFR map (INDF, TMR0, PCL, STATUS,
/// FSR, OSCCAL, GPIO) repeats in every bank (DS41236E Figure 4-4; gputils'
/// `12f509_g.lkr` declares the same range as a protected `SHAREBANK sfrs`).
/// Core architecture, not device data: identical on the 508 and 16F505.
const MIRRORED_SFRS_PIC_BASELINE: &[u16] = &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06];

include!(concat!(env!("OUT_DIR"), "/devices.rs"));

/// Resolve a device by any spelling the toolchain ecosystem uses.
///
/// The same part is written four ways: `p16f887` here, `16F887` in epic-hal's
/// manifest, `16f887` by XC8's `-mcpu`, and `PIC16F887` by MPLAB's part
/// defines. Accepting all of them is what lets the variants design keep its
/// promise that `--target` is the interface and no caller needs a board table.
///
/// `by_name` stays exact; this is the forgiving front door.
pub fn resolve(name: &str) -> Option<&'static Device> {
    let s = name.trim().to_ascii_lowercase();
    if let Some(d) = by_name(&s) {
        return Some(d);
    }
    let stem = match s.strip_prefix("pic") {
        Some(rest) => format!("p{rest}"),
        None if !s.starts_with('p') => format!("p{s}"),
        None => return None,
    };
    by_name(&stem)
}

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

    /// The bank a physical file-register address selects: `Some(n)` when the
    /// address needs a `BANKSEL`, `None` when it does not (common RAM, and the
    /// PIC14 core registers mirrored into every bank).
    ///
    /// Panics on an address the allocator must never emit: a non-canonical
    /// alias of common RAM (`0xF0` is `0x70` seen from bank 1), or a gap on a
    /// core whose banking this cannot express.
    pub fn bank_of(&self, addr: u16) -> Option<u8> {
        if let Some((lo, hi)) = self.common_ram {
            if addr >= lo && addr <= hi {
                return None;
            }
        }
        if let Some((lo, hi)) = self.fixed_retval {
            if addr >= lo && addr <= hi {
                return None;
            }
        }
        if let Some(w) = self.isr_w_shadow {
            if addr & 0x7F == w & 0x7F {
                return None;
            }
        }
        for (i, &(start, end)) in self.ram_banks.iter().enumerate() {
            if addr >= start && addr <= end {
                return Some(i as u8);
            }
        }
        if addr < self.gpr_start() {
            // SFR range, below the first GPR bank. The mirrored core
            // register block differs per core (classic PIC14's six registers,
            // PIC14E's full 0x00-0x0B per DS41364E Table 3-3, baseline's
            // whole 0x00-0x06 SFR map); any other bank-0 SFR (PORTA 0x05,
            // TMR0 0x01, ...) exists solely in bank 0, so it needs
            // RP1:RP0 = 0.
            let mirrored = match self.core {
                Core::Pic14e => MIRRORED_SFRS_PIC14E,
                Core::PicBaseline => MIRRORED_SFRS_PIC_BASELINE,
                _ => MIRRORED_SFRS,
            };
            return if mirrored.contains(&addr) {
                None
            } else {
                Some(0)
            };
        }
        match self.core {
            // PIC14 pages the 9-bit file-register address by its top two bits
            // (RP1:RP0), so a high-bank SFR such as the 887's 0x188 ANSEL is
            // banked exactly like a GPR is.
            Core::Pic14 | Core::Pic14e => {
                if let Some((lo, hi)) = self.common_ram {
                    let alias = addr & 0x7F;
                    assert!(
                        alias < lo || alias > hi,
                        "device: 0x{addr:03X} aliases common RAM 0x{alias:02X} on {}",
                        self.name
                    );
                }
                Some((addr >> 7) as u8)
            }
            Core::Pic18 => panic!(
                "device: 0x{addr:03X} is not a banked GPR address on {}",
                self.name
            ),
            Core::PicBaseline => panic!(
                // Baseline GPRs live in ram_banks and the shared window;
                // what falls through here is a gap or the protected bank-1
                // mirror of the 0x00-0x0F block (0x20-0x2F, gputils
                // `12f509_g.lkr`), which the allocator must never emit.
                "device: 0x{addr:03X} is not a banked GPR address on {}",
                self.name
            ),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FuseValue {
    pub name: &'static str,
    /// Raw bit pattern this value encodes, positioned at bit 0 and placed
    /// by `resolve_config` as `(bits << shift) & mask`. Values never set
    /// bits outside `mask >> shift`.
    pub bits: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct FuseField {
    pub name: &'static str,
    /// Offset into the region, 0-based (e.g. CONFIG4L is offset 6 in the
    /// PIC18F4550's region, which starts at `base_byte_addr`).
    pub byte_offset: u16,
    /// Bits this field owns in its byte. Usually contiguous, but may be
    /// scattered (PIC16F628A `osc` is 0x13); `shift` is the lowest set bit.
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
    /// per-device, not assumed uniform; see docs/31 §9.
    pub erased_baseline: &'static [u8],
    pub fields: &'static [FuseField],
}
