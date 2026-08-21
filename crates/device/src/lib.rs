//! Per-part memory-map and capability facts, threaded through `alloc`,
//! `isel`, `banking`, and `driver` instead of being hard-coded PIC16F877A
//! literals in each of them. See docs/29-pic18-port-design.md (§2 D-3) for
//! the design this implements. P1 adds the PIC18F4550 profile;
//! `has_hardware_multiply`/`has_tblrd`/`sfrs` still aren't added (unused so
//! far) and `access_bank` never will be  -  it's a core PIC18 invariant, not
//! a per-device fact (see docs/superpowers/plans/2026-08-18-pic18-port-p1.md). P2 populates `PIC18F4550`'s `ram_banks`/`common_ram` for real (P0/P1 left them as placeholders since nothing consumed them yet).

mod config;
pub use config::resolve_config;

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
    pub config: ConfigRegion,
}

pub const PIC16F877A: Device = Device {
    name: "p16f877a",
    core: Core::Pic14,
    flash_words: 0x2000,
    ram_banks: &[(0x20, 0x6F), (0xA0, 0xEF), (0x120, 0x16F), (0x1A0, 0x1EF)],
    common_ram: Some((0x70, 0x7F)),
    stack_depth: 8,
    interrupt_vectors: &[0x0004],
    config: ConfigRegion {
        base_byte_addr: 0x400E,
        num_bytes: 2,
        // DS39582C Register 14-1, note 1: erased value of the
        // Configuration Word is 3FFFh (low byte 0xFF, high byte 0x3F).
        erased_baseline: &[0xFF, 0x3F],
        fields: &[
            // byte_offset 0 (word bits 7:0)
            FuseField {
                name: "osc",
                byte_offset: 0,
                mask: 0x03,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "rc",
                        bits: 0b11,
                    },
                    FuseValue {
                        name: "hs",
                        bits: 0b10,
                    },
                    FuseValue {
                        name: "xt",
                        bits: 0b01,
                    },
                    FuseValue {
                        name: "lp",
                        bits: 0b00,
                    },
                ],
                default: None,
                locked: None,
            },
            FuseField {
                name: "wdt",
                byte_offset: 0,
                mask: 0x04,
                shift: 2,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None, // D-4 policy
            },
            FuseField {
                // PWRTEN: 1 = PWRT disabled, 0 = PWRT enabled (inverted).
                name: "pwrt",
                byte_offset: 0,
                mask: 0x08,
                shift: 3,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                ],
                default: Some("on"),
                locked: None, // D-4 policy: PWRT on
            },
            FuseField {
                name: "bor",
                byte_offset: 0,
                mask: 0x40,
                shift: 6,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("on"),
                locked: None, // D-4 policy
            },
            FuseField {
                name: "lvp",
                byte_offset: 0,
                mask: 0x80,
                shift: 7,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None, // D-4 policy
            },
            // byte_offset 1 (word bits 13:8; bits 15:14 do not exist,
            // the word is 14 bits, so byte 1 has no field past bit 5)
            FuseField {
                name: "cpd",
                byte_offset: 1,
                mask: 0x01,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None, // D-4 policy
            },
            FuseField {
                // WRT1:WRT0, PIC16F876A/877A decode (DS39582C Reg 14-1).
                name: "wrt",
                byte_offset: 1,
                mask: 0x06,
                shift: 1,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 0b11,
                    },
                    FuseValue {
                        name: "protect_0000_00ff",
                        bits: 0b10,
                    },
                    FuseValue {
                        name: "protect_0000_07ff",
                        bits: 0b01,
                    },
                    FuseValue {
                        name: "protect_0000_0fff",
                        bits: 0b00,
                    },
                ],
                default: Some("off"),
                locked: None, // D-4 policy
            },
            FuseField {
                name: "debug",
                byte_offset: 1,
                mask: 0x08,
                shift: 3,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None, // D-4 policy
            },
            FuseField {
                name: "cp",
                byte_offset: 1,
                mask: 0x20,
                shift: 5,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None, // D-4 policy
            },
        ],
    },
};

/// The PIC18F2455/2550/4455/4550 family (the 4550 profile specifically
/// the others share the core with less flash/RAM).
pub const PIC18F4550: Device = Device {
    name: "p18f4550",
    core: Core::Pic18,
    flash_words: 0x4000,
    // One contiguous GPR range (0x0010-0x07FF): PIC18 Access Bank
    // (0x000-0x05F) plus BSR-selected banks 0-7 (0x060-0x7FF) form
    // unbroken GPR, unlike PIC14's four banks separated by SFR holes, so a
    // single-entry table is correct here (see `Device::region_for`, which
    // already handles an arbitrary bank list generically  -  no PIC18-
    // specific allocator code is needed anywhere, only this data).
    // 0x0000-0x000F is reserved (see `common_ram` below), so GPR starts at
    // 0x0010.
    ram_banks: &[(0x0010, 0x07FF)],
    // Reserved for isel-pic18's fixed regions, bank-independent (always
    // reachable via the Access Bank's `a=0`, no `BSR` dependency),
    // mirroring PIC14's `common_ram` rationale exactly:
    //   0x0000-0x0003  the fixed `retval` region (up to 4 bytes, for an
    //                  i32 return value even though P2's own scope only
    //                  needs i8/i16)
    //   0x0004-0x000F  the fixed ISR save area (W, STATUS, BSR, FSR0L/H,
    //                  TBLPTRL/H/U, and the retval snapshot: the preempted
    //                  main's in-flight return value, P5. prologue/
    //                  epilogue; see
    //                  docs/superpowers/plans/2026-08-20-pic18-port-p5.md)
    common_ram: Some((0x0000, 0x000F)),
    stack_depth: 31,
    interrupt_vectors: &[0x0008, 0x0018],
    config: ConfigRegion {
        base_byte_addr: 0x300000,
        num_bytes: 14,
        // Confirmed against gpasm 1.5.2, 2026-08-21 (see the crate's config.rs
        // tests): every byte in this region, including the two gap addresses,
        // erases to 0xFF. DS39632E's per-register "Default/Unprogrammed Value"
        // column in Table 25-1 describes the value read back through the SFR
        // (unimplemented bits forced to 0 by hardware), not the raw flash
        // content, which is what this array represents.
        erased_baseline: &[0xFF; 14],
        fields: &[
            // ---- offset 0: CONFIG1L (0x300000), DS39632E Register 25-1 ----
            FuseField {
                name: "usbdiv",
                byte_offset: 0,
                mask: 0x20,
                shift: 5,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: None,
                locked: None, // part of the clock tree
            },
            FuseField {
                name: "cpudiv",
                byte_offset: 0,
                mask: 0x18,
                shift: 3,
                values: &[
                    FuseValue {
                        name: "div1",
                        bits: 0b00,
                    },
                    FuseValue {
                        name: "div2",
                        bits: 0b01,
                    },
                    FuseValue {
                        name: "div3",
                        bits: 0b10,
                    },
                    FuseValue {
                        name: "div4",
                        bits: 0b11,
                    },
                ],
                default: None,
                locked: None,
            },
            FuseField {
                name: "plldiv",
                byte_offset: 0,
                mask: 0x07,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "noprescale",
                        bits: 0b000,
                    }, // 4 MHz direct
                    FuseValue {
                        name: "div2",
                        bits: 0b001,
                    }, // 8 MHz input
                    FuseValue {
                        name: "div3",
                        bits: 0b010,
                    }, // 12 MHz input
                    FuseValue {
                        name: "div4",
                        bits: 0b011,
                    }, // 16 MHz input
                    FuseValue {
                        name: "div5",
                        bits: 0b100,
                    }, // 20 MHz input
                    FuseValue {
                        name: "div6",
                        bits: 0b101,
                    }, // 24 MHz input
                    FuseValue {
                        name: "div10",
                        bits: 0b110,
                    }, // 40 MHz input
                    FuseValue {
                        name: "div12",
                        bits: 0b111,
                    }, // 48 MHz input
                ],
                default: None,
                locked: None,
            },
            // ---- offset 1: CONFIG1H (0x300001), Register 25-2 ----
            FuseField {
                name: "ieso",
                byte_offset: 1,
                mask: 0x80,
                shift: 7,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "fcmen",
                byte_offset: 1,
                mask: 0x40,
                shift: 6,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                // Canonical (x=0) encoding for each named mode; DS39632E lists
                // some patterns as "111x"/"110x"/etc. where the low bit is a
                // don't-care. FOSC3:0.
                name: "osc",
                byte_offset: 1,
                mask: 0x0F,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "hspll",
                        bits: 0b1110,
                    },
                    FuseValue {
                        name: "hs",
                        bits: 0b1100,
                    },
                    FuseValue {
                        name: "inths",
                        bits: 0b1011,
                    },
                    FuseValue {
                        name: "intxt",
                        bits: 0b1010,
                    },
                    FuseValue {
                        name: "intcko",
                        bits: 0b1001,
                    },
                    FuseValue {
                        name: "intio",
                        bits: 0b1000,
                    },
                    FuseValue {
                        name: "ecpll",
                        bits: 0b0111,
                    },
                    FuseValue {
                        name: "ecpio",
                        bits: 0b0110,
                    },
                    FuseValue {
                        name: "ec",
                        bits: 0b0101,
                    },
                    FuseValue {
                        name: "ecio",
                        bits: 0b0100,
                    },
                    FuseValue {
                        name: "xtpll",
                        bits: 0b0010,
                    },
                    FuseValue {
                        name: "xt",
                        bits: 0b0000,
                    },
                ],
                default: None,
                locked: None, // the oscillator field
            },
            // ---- offset 2: CONFIG2L (0x300002), Register 25-3 ----
            FuseField {
                name: "vregen",
                byte_offset: 2,
                mask: 0x20,
                shift: 5,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None, // matches erased default
            },
            FuseField {
                name: "borv",
                byte_offset: 2,
                mask: 0x18,
                shift: 3,
                values: &[
                    FuseValue {
                        name: "minimum",
                        bits: 0b11,
                    },
                    FuseValue {
                        name: "low",
                        bits: 0b10,
                    },
                    FuseValue {
                        name: "mid",
                        bits: 0b01,
                    },
                    FuseValue {
                        name: "maximum",
                        bits: 0b00,
                    },
                ],
                default: Some("minimum"),
                locked: None, // matches erased default
            },
            FuseField {
                name: "boren",
                byte_offset: 2,
                mask: 0x06,
                shift: 1,
                values: &[
                    FuseValue {
                        name: "hw_always",
                        bits: 0b11,
                    },
                    FuseValue {
                        name: "hw_off_in_sleep",
                        bits: 0b10,
                    },
                    FuseValue {
                        name: "sw",
                        bits: 0b01,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0b00,
                    },
                ],
                default: Some("hw_always"),
                locked: None, // D-4 policy: BOR on
            },
            FuseField {
                // 1 = PWRT disabled, 0 = PWRT enabled (inverted, matches the
                // PIC16F877A's PWRTEN convention).
                name: "pwrt",
                byte_offset: 2,
                mask: 0x01,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                ],
                default: Some("on"),
                locked: None, // D-4 policy
            },
            // ---- offset 3: CONFIG2H (0x300003), Register 25-4 ----
            FuseField {
                name: "wdtps",
                byte_offset: 3,
                mask: 0x1E,
                shift: 1,
                values: &[
                    FuseValue {
                        name: "div1",
                        bits: 0b0000,
                    },
                    FuseValue {
                        name: "div2",
                        bits: 0b0001,
                    },
                    FuseValue {
                        name: "div4",
                        bits: 0b0010,
                    },
                    FuseValue {
                        name: "div8",
                        bits: 0b0011,
                    },
                    FuseValue {
                        name: "div16",
                        bits: 0b0100,
                    },
                    FuseValue {
                        name: "div32",
                        bits: 0b0101,
                    },
                    FuseValue {
                        name: "div64",
                        bits: 0b0110,
                    },
                    FuseValue {
                        name: "div128",
                        bits: 0b0111,
                    },
                    FuseValue {
                        name: "div256",
                        bits: 0b1000,
                    },
                    FuseValue {
                        name: "div512",
                        bits: 0b1001,
                    },
                    FuseValue {
                        name: "div1024",
                        bits: 0b1010,
                    },
                    FuseValue {
                        name: "div2048",
                        bits: 0b1011,
                    },
                    FuseValue {
                        name: "div4096",
                        bits: 0b1100,
                    },
                    FuseValue {
                        name: "div8192",
                        bits: 0b1101,
                    },
                    FuseValue {
                        name: "div16384",
                        bits: 0b1110,
                    },
                    FuseValue {
                        name: "div32768",
                        bits: 0b1111,
                    },
                ],
                default: Some("div32768"),
                locked: None, // matches erased default
            },
            FuseField {
                name: "wdt",
                byte_offset: 3,
                mask: 0x01,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None, // D-4 policy
            },
            // offset 4: gap (0x300004), no register, no fields
            // ---- offset 5: CONFIG3H (0x300005), Register 25-5 ----
            FuseField {
                name: "mclre",
                byte_offset: 5,
                mask: 0x80,
                shift: 7,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("on"),
                locked: None, // matches erased default
            },
            FuseField {
                name: "lpt1osc",
                byte_offset: 5,
                mask: 0x04,
                shift: 2,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "pbaden",
                byte_offset: 5,
                mask: 0x02,
                shift: 1,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("on"),
                locked: None, // matches erased default
            },
            FuseField {
                name: "ccp2mx",
                byte_offset: 5,
                mask: 0x01,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "rc1",
                        bits: 1,
                    },
                    FuseValue {
                        name: "rb3",
                        bits: 0,
                    },
                ],
                default: Some("rc1"),
                locked: None, // matches erased default
            },
            // ---- offset 6: CONFIG4L (0x300006), Register 25-6 ----
            FuseField {
                name: "debug",
                byte_offset: 6,
                mask: 0x80,
                shift: 7,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None, // D-4 policy
            },
            FuseField {
                name: "xinst",
                byte_offset: 6,
                mask: 0x40,
                shift: 6,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                ],
                default: Some("off"),
                locked: Some("off"), // codegen hazard, docs/31 D-9
            },
            FuseField {
                // DS39632E note: "Always leave this bit clear in all other
                // devices" (44-pin TQFP only). epic-cc does not model package
                // variants, so this is off unconditionally.
                name: "icprt",
                byte_offset: 6,
                mask: 0x20,
                shift: 5,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "lvp",
                byte_offset: 6,
                mask: 0x04,
                shift: 2,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None, // D-4 policy
            },
            FuseField {
                name: "stvren",
                byte_offset: 6,
                mask: 0x01,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "on",
                        bits: 1,
                    },
                    FuseValue {
                        name: "off",
                        bits: 0,
                    },
                ],
                default: Some("on"),
                locked: None, // matches erased default
            },
            // offset 7: gap (0x300007), no register, no fields
            // ---- offset 8: CONFIG5L (0x300008), Register 25-7 ----
            FuseField {
                name: "cp0",
                byte_offset: 8,
                mask: 0x01,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None, // D-4 policy
            },
            FuseField {
                name: "cp1",
                byte_offset: 8,
                mask: 0x02,
                shift: 1,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "cp2",
                byte_offset: 8,
                mask: 0x04,
                shift: 2,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "cp3",
                byte_offset: 8,
                mask: 0x08,
                shift: 3,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            // ---- offset 9: CONFIG5H (0x300009), Register 25-8 ----
            FuseField {
                name: "cpd",
                byte_offset: 9,
                mask: 0x80,
                shift: 7,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "cpb",
                byte_offset: 9,
                mask: 0x40,
                shift: 6,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            // ---- offset 10: CONFIG6L (0x30000A), Register 25-9 ----
            FuseField {
                name: "wrt0",
                byte_offset: 10,
                mask: 0x01,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "wrt1",
                byte_offset: 10,
                mask: 0x02,
                shift: 1,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "wrt2",
                byte_offset: 10,
                mask: 0x04,
                shift: 2,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "wrt3",
                byte_offset: 10,
                mask: 0x08,
                shift: 3,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            // ---- offset 11: CONFIG6H (0x30000B), Register 25-10 ----
            FuseField {
                name: "wrtc",
                byte_offset: 11,
                mask: 0x20,
                shift: 5,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "wrtb",
                byte_offset: 11,
                mask: 0x40,
                shift: 6,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "wrtd",
                byte_offset: 11,
                mask: 0x80,
                shift: 7,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            // ---- offset 12: CONFIG7L (0x30000C), Register 25-11 ----
            FuseField {
                name: "ebtr0",
                byte_offset: 12,
                mask: 0x01,
                shift: 0,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "ebtr1",
                byte_offset: 12,
                mask: 0x02,
                shift: 1,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "ebtr2",
                byte_offset: 12,
                mask: 0x04,
                shift: 2,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            FuseField {
                name: "ebtr3",
                byte_offset: 12,
                mask: 0x08,
                shift: 3,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
            // ---- offset 13: CONFIG7H (0x30000D), Register 25-12 ----
            FuseField {
                name: "ebtrb",
                byte_offset: 13,
                mask: 0x40,
                shift: 6,
                values: &[
                    FuseValue {
                        name: "off",
                        bits: 1,
                    },
                    FuseValue {
                        name: "on",
                        bits: 0,
                    },
                ],
                default: Some("off"),
                locked: None,
            },
        ],
    },
};

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
        if addr < self.gpr_start() {
            return None; // SFR range, below the first GPR bank
        }
        panic!(
            "device: 0x{addr:03X} is not a banked GPR address on {}",
            self.name
        );
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
