//! PIC16F877A (14-bit core) instruction-set simulator.
//! Owned, deterministic, cycle-counting, embeddable in `cargo test`.

use device::Device;

/// Decode Intel HEX (gpasm output) into 14-bit words, indexed by word address.
pub fn parse_hex(data: &str) -> Vec<u16> {
    let mut max_word = 8191usize; // Keeps the minimum program size for small images.
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let bytes = hex_decode(&line[1..]);
        let len = bytes[0] as usize;
        let addr = ((bytes[1] as usize) << 8) | (bytes[2] as usize);
        if bytes[3] == 0x00 {
            max_word = max_word.max(addr / 2 + len / 2);
        }
    }
    let mut words = vec![0u16; max_word + 1];
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        assert!(line.starts_with(':'), "not Intel HEX: {line}");
        let bytes = hex_decode(&line[1..]);
        let len = bytes[0] as usize;
        let addr = ((bytes[1] as usize) << 8) | (bytes[2] as usize);
        let rectype = bytes[3];
        let data = &bytes[4..4 + len];
        match rectype {
            0x00 => {
                for (i, chunk) in data.chunks(2).enumerate() {
                    let w = (chunk[0] as u16) | ((chunk[1] as u16) << 8);
                    words[addr / 2 + i] = w;
                }
            }
            0x01 => break,
            0x04 => {}
            other => panic!("unsupported HEX record type {other:#x}"),
        }
    }
    words
}

fn hex_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    while i + 1 < b.len() {
        out.push((hex_nibble(b[i]) << 4) | hex_nibble(b[i + 1]));
        i += 2;
    }
    out
}

fn hex_nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("bad hex nibble {c:#x}"),
    }
}

/// INTCON, the interrupt control register: bank-independent at 0x0B.
pub const INTCON: usize = 0x0B;
/// INTCON bit 7, the global interrupt enable.
pub const GIE: u8 = 1 << 7;
/// INTCON bit 4, the RB0/INT external interrupt enable.
pub const INTE: u8 = 1 << 4;
/// INTCON bit 1, the RB0/INT external interrupt flag.
pub const INTF: u8 = 1 << 1;
/// The 14-bit core's single interrupt vector.
pub const VECTOR: u16 = 4;

/// The 16F877A data-EEPROM register file (DS39582C chapter 4): EEDATA
/// 0x10C and EEADR 0x10D in bank 2, EECON1 0x18C and EECON2 0x18D in
/// bank 3.
const PIC14_EEDATA: usize = 0x10C;
const PIC14_EEADR: usize = 0x10D;
const PIC14_EECON1: usize = 0x18C;
const PIC14_EECON2: usize = 0x18D;

/// The 16F1938 data-EEPROM register file (DS41364E, and the SDCC
/// pic16f1938.h non-free header): EEADRL 0x191, EEDATL 0x193, EECON1
/// 0x195, EECON2 0x196, all in bank 1.
const PIC14E_EEADR: usize = 0x191;
const PIC14E_EEDATA: usize = 0x193;
const PIC14E_EECON1: usize = 0x195;
const PIC14E_EECON2: usize = 0x196;

/// The 18F4550 data-EEPROM register file (the SDCC pic18f4550.h
/// non-free header): EECON1 0xFA6, EECON2 0xFA7, EEDATA 0xFA8, EEADR
/// 0xFA9, in banked space (BSR 15).
const PIC18_EECON1: usize = 0xFA6;
const PIC18_EECON2: usize = 0xFA7;
const PIC18_EEDATA: usize = 0xFA8;
const PIC18_EEADR: usize = 0xFA9;

/// The data-EEPROM cell array shared by the three cores: 256 bytes,
/// erased (0xFF), driven through each core's EEADR/EEDATA/EECON1/EECON2
/// register file. The EECON1 bit layout does not differ across the
/// three families (DS39582C, DS41364E, DS39632E alike: RD bit 0, WR
/// bit 1, WREN bit 2), so one state machine serves all of them; only
/// the register addresses do, and the caller resolves those.
struct Eeprom {
    cells: [u8; 256],
    /// EECON2 unlock progress: 0 idle, 1 seen 0x55, 2 armed (0x55 then
    /// 0xAA, the write's required sequence).
    seq: u8,
}

impl Eeprom {
    fn new() -> Self {
        Eeprom {
            cells: [0xFF; 256],
            seq: 0,
        }
    }

    /// Handle one core store into EECON1 or EECON2. `reg` is the store's
    /// physical address, `con1`/`con2` the core's EECON1/EECON2
    /// addresses, `eeadr`/`eedata` the current register values (eedata
    /// mutable so RD can latch into it), `wren` EECON1's stored WREN
    /// bit. Returns the value to store into `reg` (RD/WR self-clear on
    /// EECON1), or None when the store does not touch the EEPROM
    /// register file.
    fn on_store(
        &mut self,
        reg: usize,
        con1: usize,
        con2: usize,
        v: u8,
        eeadr: u8,
        eedata: &mut u8,
        wren: bool,
    ) -> Option<u8> {
        if reg == con2 {
            self.seq = match (self.seq, v) {
                (0, 0x55) => 1,
                (1, 0xAA) => 2,
                _ => 0,
            };
            return Some(v);
        }
        if reg != con1 {
            return None;
        }
        // Data-EEPROM ops only: EEPGD (bit 7) or CFGS (bit 6) set
        // targets program flash or config space, which no compiled
        // corpus program drives through this window; store as-is. Bit 5
        // is LWLO on the Enhanced core, a legitimate data-EEPROM
        // modifier, so it must not divert here.
        if v & 0xC0 != 0 {
            self.seq = 0;
            return Some(v);
        }
        let mut out = v;
        if v & 0x01 != 0 {
            // RD latches the addressed cell into EEDATA and self-clears.
            *eedata = self.cells[eeadr as usize];
            out &= !0x01;
        }
        if v & 0x02 != 0 {
            // WR commits only after the unlock sequence with WREN set,
            // then self-clears.
            if wren && self.seq == 2 {
                self.cells[eeadr as usize] = *eedata;
            }
            out &= !0x02;
            self.seq = 0;
        }
        Some(out)
    }
}

pub struct Pic14 {
    /// Supplies the GPR map. A direct operand is banked GPR when its physical
    /// address falls inside one of this device's `ram_banks`, which is not the
    /// same as a fixed operand window: bank 2 and 3 SFRs are shorter than bank
    /// 0's, so their GPR starts below 0x20.
    device: &'static Device,
    prog: Vec<u16>,
    ram: [u8; 512],
    w: u8,
    pc: u16,
    stack: Vec<u16>,
    halted: bool,
    /// A latched interrupt request awaiting GIE + INTE. Set by
    /// `request_interrupt`, consumed when the interrupt is taken.
    pending: bool,
    /// The data-EEPROM cell array behind the EEADR/EEDATA/EECON1/EECON2
    /// register file.
    eeprom: Eeprom,
}

impl Pic14 {
    /// A simulator on the canonical PIC14 geometry (`PIC16F877A`), for callers
    /// that only execute instructions without depending on GPR placement in
    /// a bank. Use [`Pic14::with_device`] for anything that does: a part whose
    /// banks differ models wrongly here, silently.
    pub fn new(prog: Vec<u16>) -> Self {
        Self::with_device(&device::PIC16F877A, prog)
    }

    /// A simulator on `device`'s real memory map.
    pub fn with_device(device: &'static Device, prog: Vec<u16>) -> Self {
        Pic14 {
            device,
            prog,
            ram: [0; 512],
            w: 0,
            pc: 0,
            stack: Vec::new(),
            halted: false,
            pending: false,
            eeprom: Eeprom::new(),
        }
    }
    pub fn ram(&self) -> &[u8; 512] {
        &self.ram
    }
    pub fn ram_mut(&mut self) -> &mut [u8; 512] {
        &mut self.ram
    }
    pub fn eeprom(&self) -> &[u8; 256] {
        &self.eeprom.cells
    }
    /// Route a direct store at physical address `phys` through the
    /// data-EEPROM register file: None when the address is not one of
    /// the EEPROM registers, else the value to store (possibly
    /// RD/WR-adjusted).
    fn ee_store(&mut self, phys: usize, v: u8) -> Option<u8> {
        if phys != PIC14_EECON1 && phys != PIC14_EECON2 {
            return None;
        }
        let eeadr = self.ram[PIC14_EEADR];
        let wren = self.ram[PIC14_EECON1] & 0x04 != 0;
        self.eeprom.on_store(
            phys,
            PIC14_EECON1,
            PIC14_EECON2,
            v,
            eeadr,
            &mut self.ram[PIC14_EEDATA],
            wren,
        )
    }
    pub fn w(&self) -> u8 {
        self.w
    }
    pub fn pc(&self) -> u16 {
        self.pc
    }
    pub fn halted(&self) -> bool {
        self.halted
    }
    /// Fire the F877A's single interrupt immediately, bypassing GIE and the
    /// enable bits: push the return address and jump to the vector. The
    /// unconditional test hook: use it to place an interrupt at an exact
    /// program counter without modelling INTCON.
    ///
    /// `fire_interrupt` is called BETWEEN steps, so `pc` addresses an
    /// instruction that still awaits execution: the return address is `pc`
    /// itself, and RETFIE resumes by running it. (Pushing `pc + 1` would
    /// silently drop that instruction.)
    pub fn fire_interrupt(&mut self) {
        self.enter_isr();
    }
    /// Request the interrupt through the modelled path: latch it and set
    /// INTF. It is taken at the next step boundary at which GIE and INTE are
    /// both set, so a program that masks interrupts keeps it pending until
    /// it unmasks. The latch is consumed on entry, so a handler that leaves
    /// INTF set still runs once rather than looping.
    pub fn request_interrupt(&mut self) {
        self.ram[INTCON] |= INTF;
        self.pending = true;
    }
    /// Whether a requested interrupt remains latched and untaken.
    pub fn interrupt_pending(&self) -> bool {
        self.pending
    }
    /// Push the return address, clear GIE (hardware does this on entry so
    /// the handler is not immediately re-entered) and vector.
    fn enter_isr(&mut self) {
        self.stack.push(self.pc);
        self.ram[INTCON] &= !GIE;
        self.pc = VECTOR;
    }
    /// A latched request whose source and global enables are both set.
    fn interrupt_ready(&self) -> bool {
        self.pending && self.ram[INTCON] & GIE != 0 && self.ram[INTCON] & INTE != 0
    }
    pub fn run(&mut self, max_steps: usize) -> usize {
        let mut steps = 0;
        while !self.halted && steps < max_steps {
            self.step();
            steps += 1;
        }
        steps
    }
    pub fn step(&mut self) {
        // Interrupts are recognised at an instruction boundary: a latched,
        // enabled request vectors instead of executing this instruction,
        // which then runs on return.
        if self.interrupt_ready() {
            self.pending = false;
            self.enter_isr();
            return; // vectoring costs its own cycle; the handler runs next
        }
        let word = self.prog[self.pc as usize];
        let pc = self.pc;
        let next = match (word >> 12) & 0x3 {
            0 => self.exec_byte(pc, word),
            1 => self.exec_bit(pc, word),
            2 => self.exec_call_goto(pc, word),
            3 => self.exec_literal(pc, word),
            _ => unreachable!(),
        };
        self.pc = next;
        if self.pc as usize >= self.prog.len() {
            self.halted = true;
        }
    }

    fn set_z(&mut self, v: u8) {
        if v == 0 {
            self.ram[3] |= 0b100;
        } else {
            self.ram[3] &= !0b100;
        }
    }
    fn set_c(&mut self, c: bool) {
        if c {
            self.ram[3] |= 0b001;
        } else {
            self.ram[3] &= !0b001;
        }
    }
    fn set_dc(&mut self, c: bool) {
        if c {
            self.ram[3] |= 0b010;
        } else {
            self.ram[3] &= !0b010;
        }
    }
    // Resolve INDF's target: IRP (STATUS bit 7) selects the upper/lower 256;
    // the common region 0x70-0x7F is mirrored in all banks and ignores IRP.
    fn indirect_addr(&self) -> usize {
        let fsr = self.ram[0x04] as usize;
        if (0x70..=0x7F).contains(&fsr) {
            fsr // common region (0x70-0x7F), mirrored in all banks
        } else {
            let base = if self.ram[3] & 0x80 != 0 { 0x100 } else { 0 };
            base + fsr
        }
    }
    // Bank base for direct operands: bank = STATUS<6:5> (RP1:RP0).
    fn bank_base(&self) -> usize {
        ((self.ram[3] >> 5) & 0x3) as usize * 0x80
    }

    /// The physical address of a direct operand when it names banked GPR, or
    /// `None` when it names common RAM or a bank-independent SFR (both of
    /// which the compiler addresses by their bank-0 offset).
    fn banked_addr(&self, f: usize) -> Option<usize> {
        if let Some((lo, hi)) = self.device.common_ram {
            if f >= lo as usize && f <= hi as usize {
                return None;
            }
        }
        if let Some((lo, hi)) = self.device.fixed_retval {
            if f >= lo as usize && f <= hi as usize {
                return None;
            }
        }
        // The six core registers (INDF, PCL, STATUS, FSR, PCLATH, INTCON)
        // are mirrored into every bank and addressed by their bank-0
        // offset; every other direct operand is paged by RP1:RP0, so its
        // physical address is `f + bank_base` even when that lands in a
        // banked SFR window (PIE1 at 0x8C, PIR1 at 0x0C, ...) rather than
        // a GPR bank (epic-cc#173).
        if matches!(f, 0x00 | 0x02 | 0x03 | 0x04 | 0x0A | 0x0B) {
            return None;
        }
        Some(f + self.bank_base())
    }
    fn read_f(&self, f: usize) -> u8 {
        match f {
            0x00 => self.ram[self.indirect_addr()], // INDF -> RAM[FSR] via IRP
            0x02 => (self.pc & 0xFF) as u8,         // PCL
            _ => match self.banked_addr(f) {
                Some(phys) => self.ram[phys],
                // SFR 0x01-0x1F (bank-independent) and common 0x70-0x7F
                None => self.ram[f],
            },
        }
    }
    fn write_f(&mut self, f: usize, v: u8) {
        match f {
            0x00 => {
                let addr = self.indirect_addr();
                let v = self.ee_store(addr, v).unwrap_or(v);
                self.ram[addr] = v; // INDF -> RAM[FSR] via IRP
            }
            _ => match self.banked_addr(f) {
                Some(phys) => {
                    let v = self.ee_store(phys, v).unwrap_or(v);
                    self.ram[phys] = v;
                }
                // SFR 0x01-0x1F (bank-independent) and common 0x70-0x7F
                None => self.ram[f] = v,
            },
        }
    }
    fn write_d(&mut self, d: u16, f: usize, r: u8) {
        if d == 1 {
            self.write_f(f, r);
        } else {
            self.w = r;
        }
    }
    fn add_flags(&mut self, a: u8, b: u8, r: u8) {
        self.set_z(r);
        self.set_c((a as u16 + b as u16) > 0xFF);
        self.set_dc(((a & 0x0F) as u16 + (b & 0x0F) as u16) > 0x0F);
    }
    fn rlf(&mut self, v: u8) -> u8 {
        let cin = if self.ram[3] & 0b001 != 0 { 1 } else { 0 };
        let cout = v >> 7;
        let r = (v << 1) | cin;
        self.set_c(cout != 0);
        r
    }
    fn rrf(&mut self, v: u8) -> u8 {
        let cin = if self.ram[3] & 0b001 != 0 { 0x80 } else { 0 };
        let cout = v & 1;
        let r = (v >> 1) | cin;
        self.set_c(cout != 0);
        r
    }
    fn pop_return(&mut self) -> u16 {
        self.stack.pop().unwrap_or(0)
    }

    fn exec_byte(&mut self, pc: u16, word: u16) -> u16 {
        match word {
            0x0000 => return pc + 1,            // NOP
            0x0008 => return self.pop_return(), // RETURN
            0x0009 => {
                self.ram[INTCON] |= GIE; // RETFIE re-enables interrupts
                return self.pop_return();
            }
            0x0064 => return pc + 1, // CLRWDT
            0x0063 => {
                self.halted = true; // SLEEP
                return pc;
            }
            _ => {}
        }
        let d = (word >> 7) & 1;
        let f = (word & 0x7F) as usize;
        let op6 = (word >> 8) & 0x3F;
        match op6 {
            0x07 => {
                let v = self.read_f(f);
                let r = self.w.wrapping_add(v);
                self.add_flags(self.w, v, r);
                self.write_d(d, f, r);
            }
            0x05 => {
                let r = self.w & self.read_f(f);
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x01 => {
                if d == 1 {
                    self.write_f(f, 0);
                } else {
                    self.w = 0;
                }
                self.set_z(0);
            }
            0x08 => {
                let r = self.read_f(f);
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x00 => {
                if d == 1 {
                    if f == 0x02 {
                        let pclath = (self.ram[0x0A] as u16) & 0x1F;
                        return (pclath << 8) | (self.w as u16);
                    }
                    self.write_f(f, self.w);
                }
            }
            0x02 => {
                let v = self.read_f(f);
                let r = v.wrapping_sub(self.w);
                self.set_z(r);
                self.set_c(v >= self.w);
                self.set_dc((v & 0x0F) >= (self.w & 0x0F));
                self.write_d(d, f, r);
            }
            0x09 => {
                let r = !self.read_f(f); // COMF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x03 => {
                let r = self.read_f(f).wrapping_sub(1); // DECF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x0B => {
                let r = self.read_f(f).wrapping_sub(1); // DECFSZ
                self.set_z(r);
                self.write_d(d, f, r);
                if r == 0 {
                    return pc + 2;
                }
            }
            0x0A => {
                let r = self.read_f(f).wrapping_add(1); // INCF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x0F => {
                let r = self.read_f(f).wrapping_add(1); // INCFSZ
                self.set_z(r);
                self.write_d(d, f, r);
                if r == 0 {
                    return pc + 2;
                }
            }
            0x04 => {
                let r = self.w | self.read_f(f); // IORWF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x0D => {
                let r = self.rlf(self.read_f(f)); // RLF
                self.write_d(d, f, r);
            }
            0x0C => {
                let r = self.rrf(self.read_f(f)); // RRF
                self.write_d(d, f, r);
            }
            0x06 => {
                let r = self.w ^ self.read_f(f); // XORWF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x0E => {
                let v = self.read_f(f); // SWAPF
                let r = (v << 4) | (v >> 4);
                self.write_d(d, f, r);
            }
            other => panic!("byte opcode {other:#x} not yet implemented"),
        }
        pc + 1
    }
    fn exec_bit(&mut self, pc: u16, word: u16) -> u16 {
        let b = ((word >> 7) & 0x7) as u8;
        let f = (word & 0x7F) as usize;
        match (word >> 10) & 0x3 {
            0 => self.write_f(f, self.read_f(f) & !(1 << b)), // BCF
            1 => self.write_f(f, self.read_f(f) | (1 << b)),  // BSF
            2 => {
                if self.read_f(f) & (1 << b) == 0 {
                    return pc + 2; // BTFSC skip if clear
                }
            }
            3 => {
                if self.read_f(f) & (1 << b) != 0 {
                    return pc + 2; // BTFSS skip if set
                }
            }
            _ => unreachable!(),
        }
        pc + 1
    }
    fn exec_call_goto(&mut self, pc: u16, word: u16) -> u16 {
        let k = word & 0x7FF;
        // PCLATH<4:3> -> PC<12:11>; PCLATH is NOT modified by CALL/GOTO.
        let target = ((self.ram[0x0A] as u16 & 0x18) << 8) | k;
        if word & 0x0800 != 0 {
            target // GOTO
        } else {
            self.stack.push(pc + 1); // CALL
            target
        }
    }
    fn exec_literal(&mut self, pc: u16, word: u16) -> u16 {
        let k = (word & 0xFF) as u8;
        match (word >> 8) & 0xF {
            0xE | 0xF => {
                let r = self.w.wrapping_add(k);
                self.add_flags(self.w, k, r);
                self.w = r;
            }
            0x9 => {
                self.w &= k;
                self.set_z(self.w);
            }
            0x8 => {
                self.w |= k;
                self.set_z(self.w);
            }
            0xA => {
                self.w ^= k;
                self.set_z(self.w);
            }
            0xC | 0xD => {
                let r = k.wrapping_sub(self.w);
                self.set_z(r);
                self.set_c(k >= self.w);
                self.set_dc((k & 0x0F) >= (self.w & 0x0F));
                self.w = r;
            }
            0x0..=0x3 => self.w = k, // MOVLW
            0x4..=0x7 => {
                self.w = k; // RETLW
                let ret = self.pop_return();
                return ret;
            }
            _ => unreachable!(),
        }
        pc + 1
    }
}

/// Why `run_until` stopped: the target address is the next instruction,
/// the program ran past its last word, or the step cap expired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Reached,
    Halted,
    Capped,
}

impl Pic14 {
    /// STATUS, the flag/bank register at 0x03.
    pub fn status(&self) -> u8 {
        self.ram[3]
    }

    /// The active GPR bank (STATUS RP1:RP0), 0-3.
    pub fn bank(&self) -> u8 {
        ((self.ram[3] >> 5) & 0b11) as u8
    }

    /// Select the GPR bank (writes RP1:RP0, the other STATUS bits keep
    /// their values).
    pub fn set_bank(&mut self, bank: u8) {
        self.ram[3] = (self.ram[3] & !(0b11 << 5)) | ((bank & 0b11) << 5);
    }

    /// FSR, the indirect-address register at 0x04.
    pub fn fsr(&self) -> u8 {
        self.ram[0x04]
    }

    /// PCLATH, the PC latch at 0x0A.
    pub fn pclath(&self) -> u8 {
        self.ram[0x0A]
    }

    /// INTCON, the interrupt control register at 0x0B.
    pub fn intcon(&self) -> u8 {
        self.ram[INTCON]
    }

    /// Set the working register.
    pub fn set_w(&mut self, v: u8) {
        self.w = v;
    }

    /// Set the program counter. Execution resumes from the written
    /// address; the halted flag tracks it, so writing an in-range
    /// address resumes a program that ran off its end.
    pub fn set_pc(&mut self, addr: u16) {
        self.pc = addr;
        self.halted = addr as usize >= self.prog.len();
    }

    /// Run until `pc` reaches `target`, the program halts, or
    /// `max_steps` instructions have executed. The breakpoint primitive:
    /// the adapter owns the breakpoint set and calls this per stop, so
    /// the sim keeps no breakpoint state of its own.
    pub fn run_until(&mut self, target: u16, max_steps: usize) -> StopReason {
        let mut steps = 0;
        loop {
            if self.pc == target {
                return StopReason::Reached;
            }
            if self.halted {
                return StopReason::Halted;
            }
            if steps >= max_steps {
                return StopReason::Capped;
            }
            self.step();
            steps += 1;
        }
    }

    /// Read `len` bytes of the data image starting at direct-operand
    /// address `addr`, each byte banked-resolved the way `banked_addr`
    /// resolves the machine's own direct operands (so `read_mem(0x20, 4)`
    /// with bank 1 selected reads 0xA0..0xA3). INDF reads and PCL reads
    /// keep their machine semantics; a PCL byte written here is stored,
    /// not loaded into the PC: use `set_pc` for that. Panics when `addr`
    /// leaves the 7-bit direct-operand space: no ISA operand can name
    /// it, so there is no machine-faithful resolution.
    pub fn read_mem(&self, addr: u8, len: usize) -> Vec<u8> {
        self.check_operand_range(addr);
        (0..len)
            .map(|i| self.read_f((addr.wrapping_add(i as u8) & 0x7F) as usize))
            .collect()
    }

    /// Write bytes into the data image, resolved like `read_mem`.
    pub fn write_mem(&mut self, addr: u8, bytes: &[u8]) {
        self.check_operand_range(addr);
        for (i, &b) in bytes.iter().enumerate() {
            self.write_f((addr.wrapping_add(i as u8) & 0x7F) as usize, b);
        }
    }

    /// A span must start in the 7-bit direct-operand space; the span
    /// itself wraps per byte, like consecutive operands.
    fn check_operand_range(&self, addr: u8) {
        assert!(
            addr <= 0x7F,
            "read_mem/write_mem: {addr:#04X} is outside the 7-bit direct-operand space"
        );
    }

    /// Read one program-flash word.
    pub fn read_prog_word(&self, addr: u16) -> Option<u16> {
        self.prog.get(addr as usize).copied()
    }
}

/// The three FSR address regions of the Enhanced Mid-range core
/// (DS41364E section 3.5): traditional data memory 0x000-0xFFF, the
/// linear alias 0x2000-0x29AF (banks 0-30, 80 GPR bytes per bank, the 16
/// common bytes excluded), and program flash from 0x8000 when FSRnH's MSb
/// is set (low 8 bits of each word readable through INDF, read-only, one
/// extra cycle per access).
pub struct Pic14e {
    prog: Vec<u16>,
    /// 4096 bytes of data memory (32 banks x 128 bytes, DS41364E
    /// section 3.2).
    ram: [u8; 4096],
    w: u8,
    /// Word address; the PC is 15 bits, PCLATH supplies PC<14:8>.
    pc: u16,
    stack: Vec<u16>,
    halted: bool,
    /// A latched interrupt request awaiting GIE + INTE, mirroring `Pic14`.
    pending: bool,
    /// The hardware shadow-register context save (DS41364E section 7.5):
    /// W, STATUS (except TO/PD), BSR, FSR0L/H, FSR1L/H, PCLATH are
    /// snapshotted on interrupt entry and restored on RETFIE. The compiler
    /// emits no manual save/restore for them, so the sim models the shadow
    /// save to stay faithful.
    shadow: [u8; 8],
    /// Extra cycle owed by an INDF access to the program-flash region
    /// (DS41364E section 3.5.3 note 2): the step budget consumes it before
    /// the next instruction executes.
    cycle_debt: u8,
    /// The data-EEPROM cell array behind the EEADR/EEDAT/EECON1/EECON2
    /// register file.
    eeprom: Eeprom,
}

impl Pic14e {
    /// A simulator on `device`'s memory map. The bank map, the linear
    /// region and the flash window are per-core constants on this core
    /// (docs/33 §D-2), so only the `Core::Pic14e` contract is checked, not
    /// per-device fields.
    pub fn with_device(device: &Device, prog: Vec<u16>) -> Self {
        assert_eq!(
            device.core,
            device::Core::Pic14e,
            "sim(pic14e): {} is not a pic14e device",
            device.name
        );
        Pic14e {
            prog,
            ram: [0; 4096],
            w: 0,
            pc: 0,
            stack: Vec::new(),
            halted: false,
            eeprom: Eeprom::new(),
            pending: false,
            shadow: [0; 8],
            cycle_debt: 0,
        }
    }
    pub fn ram(&self) -> &[u8; 4096] {
        &self.ram
    }
    pub fn ram_mut(&mut self) -> &mut [u8; 4096] {
        &mut self.ram
    }
    pub fn eeprom(&self) -> &[u8; 256] {
        &self.eeprom.cells
    }
    /// Route a direct store at physical address `a` through the
    /// data-EEPROM register file (same contract as `Pic14::ee_store`).
    fn ee_store(&mut self, a: usize, v: u8) -> Option<u8> {
        if a != PIC14E_EECON1 && a != PIC14E_EECON2 {
            return None;
        }
        let eeadr = self.ram[PIC14E_EEADR];
        let wren = self.ram[PIC14E_EECON1] & 0x04 != 0;
        self.eeprom.on_store(
            a,
            PIC14E_EECON1,
            PIC14E_EECON2,
            v,
            eeadr,
            &mut self.ram[PIC14E_EEDATA],
            wren,
        )
    }
    pub fn w(&self) -> u8 {
        self.w
    }
    pub fn pc(&self) -> u16 {
        self.pc
    }
    pub fn halted(&self) -> bool {
        self.halted
    }
    pub fn fire_interrupt(&mut self) {
        self.enter_isr();
    }
    pub fn request_interrupt(&mut self) {
        self.ram[INTCON] |= INTF;
        self.pending = true;
    }
    pub fn interrupt_pending(&self) -> bool {
        self.pending
    }
    fn enter_isr(&mut self) {
        // Hardware shadow context save (DS41364E section 7.5): W,
        // STATUS (except TO and PD), BSR, FSR0, FSR1 and PCLATH. The
        // compiler relies on it (no manual save/restore), so the sim
        // restores these at RETFIE.
        self.shadow[0] = self.w;
        self.shadow[1] = self.ram[3] & !0x18; // STATUS minus TO (bit 4) and PD (bit 3)
        self.shadow[2] = self.ram[8]; // BSR
        for (s, r) in self.shadow[3..].iter_mut().zip([4u8, 5, 6, 7, 10]) {
            *s = self.ram[usize::from(r)];
        } // FSR0L, FSR0H, FSR1L, FSR1H, PCLATH
        self.stack.push(self.pc);
        self.ram[INTCON] &= !GIE;
        self.pc = VECTOR;
    }
    fn interrupt_ready(&self) -> bool {
        self.pending && self.ram[INTCON] & GIE != 0 && self.ram[INTCON] & INTE != 0
    }
    pub fn run(&mut self, max_steps: usize) -> usize {
        let mut steps = 0;
        while !self.halted && steps < max_steps {
            self.step();
            steps += 1;
        }
        steps
    }
    pub fn step(&mut self) {
        // A flash INDF access costs an extra cycle: consume it before the
        // next instruction (DS41364E section 3.5.3).
        if self.cycle_debt > 0 {
            self.cycle_debt -= 1;
            return;
        }
        if self.interrupt_ready() {
            self.pending = false;
            self.enter_isr();
            return;
        }
        let word = self.prog[self.pc as usize];
        let pc = self.pc;
        let next = match (word >> 12) & 0x3 {
            0 => self.exec_byte(pc, word),
            1 => self.exec_bit(pc, word),
            2 => self.exec_call_goto(pc, word),
            3 => self.exec_literal(pc, word),
            _ => unreachable!(),
        };
        self.pc = next;
        if self.pc as usize >= self.prog.len() {
            self.halted = true;
        }
    }

    fn set_z(&mut self, v: u8) {
        if v == 0 {
            self.ram[3] |= 0b100;
        } else {
            self.ram[3] &= !0b100;
        }
    }
    fn set_c(&mut self, c: bool) {
        if c {
            self.ram[3] |= 0b001;
        } else {
            self.ram[3] &= !0b001;
        }
    }

    /// The physical data-memory address an FSR selects. Program flash is
    /// read-only, so no write path resolves here: `write_ram` panics
    /// on an FSR address in the flash region.
    fn indirect_addr(&self, fsr: u16) -> usize {
        if fsr & 0x8000 != 0 {
            // Program flash: the lower 15 bits are the word address, only
            // the low 8 bits are readable (DS41364E section 3.5.3).
            usize::from(fsr & 0x7FFF)
        } else if (0x2000..=0x29AF).contains(&fsr) {
            // Linear alias: bank * 80 + (GPR offset - 0x20), banks 0-30
            // (DS41364E section 3.5.2).
            let off = usize::from(fsr - 0x2000);
            let bank = off / 80;
            (bank * 0x80) + 0x20 + (off % 80)
        } else {
            // Traditional data memory, 0x000-0xFFF.
            usize::from(fsr)
        }
    }

    fn read_ram(&mut self, fsr: u16) -> u8 {
        let a = self.indirect_addr(fsr);
        if fsr & 0x8000 != 0 {
            // One extra cycle per flash access; the low 8 bits of the word.
            self.cycle_debt = 1;
            return (self.prog[a] & 0xFF) as u8;
        }
        self.ram[a]
    }

    /// The physical address of a direct operand: the mirrored core
    /// registers (0x00-0x0B, DS41364E Table 3-3) and the common RAM block
    /// (0x70-0x7F, DS41364E section 3.2.4) are addressable from any bank
    /// at their physical address regardless of BSR; everything else is
    /// paged by BSR.
    fn direct_addr(&self, f: usize) -> usize {
        if f <= 0x0B || (0x70..=0x7F).contains(&f) {
            f
        } else {
            ((self.ram[0x08] as usize & 0x1F) << 7) | f
        }
    }
    fn read_f(&mut self, f: usize) -> u8 {
        match f {
            0x00 => self.read_ram(self.fsr(0)), // INDF0
            0x01 => self.read_ram(self.fsr(1)), // INDF1
            0x02 => (self.pc & 0xFF) as u8,     // PCL
            0x09 => self.w,                     // WREG
            _ => self.ram[self.direct_addr(f)],
        }
    }
    fn write_ram(&mut self, fsr: u16, v: u8) {
        assert!(
            fsr & 0x8000 == 0,
            "sim(pic14e): program flash is read-only (FSR 0x{fsr:04X})"
        );
        let a = self.indirect_addr(fsr);
        let v = self.ee_store(a, v).unwrap_or(v);
        self.ram[a] = v;
    }
    fn write_f(&mut self, f: usize, v: u8) {
        match f {
            0x00 => self.write_ram(self.fsr(0), v), // INDF0
            0x01 => self.write_ram(self.fsr(1), v), // INDF1
            0x02 => {
                // PCL write: the whole PC changes to PCLATH<6:0>:v.
                self.pc = ((self.ram[0x0A] as u16 & 0x7F) << 8) | v as u16;
            }
            0x03 => {
                // STATUS bits 7-5 are unimplemented (read as 0) and TO/PD
                // are not writable (DS41364E Register 3-1): a write can
                // only reach Z/DC/C.
                self.ram[3] = (self.ram[3] & 0x18) | (v & 0x07);
            }
            0x09 => self.w = v, // WREG
            _ => {
                let a = self.direct_addr(f);
                let v = self.ee_store(a, v).unwrap_or(v);
                self.ram[a] = v;
            }
        }
    }
    /// The current FSR0/FSR1 value as a 16-bit address.
    fn fsr(&self, n: usize) -> u16 {
        u16::from(self.ram[4 + 2 * n]) | (u16::from(self.ram[5 + 2 * n]) << 8)
    }
    fn set_fsr(&mut self, n: usize, v: u16) {
        self.ram[4 + 2 * n] = (v & 0xFF) as u8;
        self.ram[5 + 2 * n] = (v >> 8) as u8;
    }
    fn write_d(&mut self, d: u16, f: usize, r: u8) {
        if d == 1 {
            self.write_f(f, r);
        } else {
            self.w = r;
        }
    }
    fn pop_return(&mut self) -> u16 {
        self.stack.pop().unwrap_or(0)
    }
    fn set_dc(&mut self, c: bool) {
        if c {
            self.ram[3] |= 0b010;
        } else {
            self.ram[3] &= !0b010;
        }
    }
    fn add_flags(&mut self, a: u8, b: u8, r: u8) {
        self.set_z(r);
        self.set_c((a as u16 + b as u16) > 0xFF);
        self.set_dc(((a & 0x0F) as u16 + (b & 0x0F) as u16) > 0x0F);
    }

    fn exec_byte(&mut self, pc: u16, word: u16) -> u16 {
        match word {
            0x0000 => return pc + 1, // NOP
            0x0001 => {
                // RESET: reinitialize the core (PC 0, W 0, stack emptied),
                // matching `Pic18`'s RESET model.
                self.w = 0;
                self.stack.clear();
                return 0;
            }
            0x0008 => return self.pop_return(), // RETURN
            0x0009 => {
                // RETFIE: restore the hardware shadow context save
                // (DS41364E section 7.5) and re-enable interrupts.
                self.w = self.shadow[0];
                self.ram[3] = (self.ram[3] & 0x18) | (self.shadow[1] & 0x07); // STATUS minus TO/PD
                self.ram[8] = self.shadow[2]; // BSR
                for (s, r) in self.shadow[3..].iter().zip([4u8, 5, 6, 7, 10]) {
                    self.ram[usize::from(r)] = *s;
                } // FSR0L, FSR0H, FSR1L, FSR1H, PCLATH
                self.ram[INTCON] |= GIE; // RETFIE re-enables interrupts
                return self.pop_return();
            }
            0x000A => {
                // CALLW: PCL = W, PCH = PCLATH (DS41364E section 3.3.3).
                let target = ((self.ram[0x0A] as u16 & 0x7F) << 8) | self.w as u16;
                self.stack.push(pc + 1);
                return target;
            }
            0x000B => {
                // BRW: PC = PC + 1 + W (DS41364E section 3.3.4).
                return pc + 1 + self.w as u16;
            }
            0x0062 => {
                // OPTION: W -> OPTION_REG (0x095), written by its absolute
                // address regardless of BSR.
                self.ram[0x95] = self.w;
                return pc + 1;
            }
            0x0063 => {
                self.halted = true; // SLEEP
                return pc;
            }
            0x0064 => return pc + 1, // CLRWDT
            _ if (0x0010..=0x001F).contains(&word) => {
                // MOVIW/MOVWI with pre/post inc/dec: 00 0000 0001 dnmm.
                let n = ((word >> 2) & 1) as usize;
                let mm = (word & 0x3) as u8;
                let to_w = word & 0x08 == 0;
                let fsr = self.fsr(n);
                // ++FSRn = 00, --FSRn = 01, FSRn++ = 10, FSRn-- = 11.
                let (eff, after) = match mm {
                    0 => (fsr.wrapping_add(1), fsr.wrapping_add(1)),
                    1 => (fsr.wrapping_sub(1), fsr.wrapping_sub(1)),
                    2 => (fsr, fsr.wrapping_add(1)),
                    _ => (fsr, fsr.wrapping_sub(1)),
                };
                let v = self.read_ram(eff);
                self.set_fsr(n, after);
                if to_w {
                    self.set_z(v);
                    self.w = v;
                } else {
                    self.write_ram(eff, self.w);
                }
                return pc + 1;
            }
            _ if (0x0020..=0x003F).contains(&word) => {
                self.ram[0x08] = (word & 0x1F) as u8; // MOVLB k
                return pc + 1;
            }
            _ if (0x0060..=0x0067).contains(&word) => {
                // TRIS f: W -> TRISx (f = 5, 6, 7 at 0x08C-0x08E, DS41364E
                // Register 12-3); other f values are not a documented TRIS
                // register, and 0x0062/0x0063/0x0064 (OPTION/SLEEP/CLRWDT)
                // are pre-empted above.
                let f = word & 0x7;
                assert!(
                    (5..=7).contains(&f),
                    "sim(pic14e): TRIS {f} has no TRIS register (only TRISA/B/C exist)"
                );
                self.ram[0x8C + (f - 5) as usize] = self.w;
                return pc + 1;
            }
            _ => {}
        }
        let d = (word >> 7) & 1;
        let f = (word & 0x7F) as usize;
        let op6 = (word >> 8) & 0x3F;
        match op6 {
            0x07 => {
                let v = self.read_f(f);
                let r = self.w.wrapping_add(v);
                self.add_flags(self.w, v, r);
                self.write_d(d, f, r);
            }
            0x05 => {
                let r = self.w & self.read_f(f);
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x01 => {
                if d == 1 {
                    self.write_f(f, 0);
                } else {
                    self.w = 0;
                }
                self.set_z(0);
            }
            0x08 => {
                let r = self.read_f(f);
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x00 => {
                if d == 1 {
                    if f == 0x02 {
                        // MOVWF PCL changes the whole PC, handled by the
                        // PCL write path.
                        let pclath = (self.ram[0x0A] as u16) & 0x7F;
                        return (pclath << 8) | (self.w as u16);
                    }
                    self.write_f(f, self.w);
                }
            }
            0x02 => {
                let v = self.read_f(f);
                let r = v.wrapping_sub(self.w);
                self.set_z(r);
                self.set_c(v >= self.w);
                self.set_dc((v & 0x0F) >= (self.w & 0x0F));
                self.write_d(d, f, r);
            }
            0x09 => {
                let r = !self.read_f(f); // COMF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x03 => {
                let r = self.read_f(f).wrapping_sub(1); // DECF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x0B => {
                let r = self.read_f(f).wrapping_sub(1); // DECFSZ
                self.set_z(r);
                self.write_d(d, f, r);
                if r == 0 {
                    return pc + 2;
                }
            }
            0x0A => {
                let r = self.read_f(f).wrapping_add(1); // INCF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x0F => {
                let r = self.read_f(f).wrapping_add(1); // INCFSZ
                self.set_z(r);
                self.write_d(d, f, r);
                if r == 0 {
                    return pc + 2;
                }
            }
            0x04 => {
                let r = self.w | self.read_f(f); // IORWF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x0D => {
                // RLF through carry; the carry bit is bit 0 of the shifted
                // value and the shifted-out bit becomes C.
                let cin = if self.ram[3] & 0b001 != 0 { 1 } else { 0 };
                let v = self.read_f(f);
                let r = (v << 1) | cin;
                self.set_c(v & 0x80 != 0);
                self.write_d(d, f, r);
            }
            0x0C => {
                let cin = if self.ram[3] & 0b001 != 0 { 0x80 } else { 0 };
                let v = self.read_f(f);
                let r = (v >> 1) | cin;
                self.set_c(v & 0x01 != 0);
                self.write_d(d, f, r);
            }
            0x06 => {
                let r = self.w ^ self.read_f(f); // XORWF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x0E => {
                let v = self.read_f(f); // SWAPF
                let r = (v << 4) | (v >> 4);
                self.write_d(d, f, r);
            }
            other => panic!("sim(pic14e): byte opcode {other:#x} not yet implemented"),
        }
        pc + 1
    }

    fn exec_bit(&mut self, pc: u16, word: u16) -> u16 {
        let b = ((word >> 7) & 0x7) as u8;
        let f = (word & 0x7F) as usize;
        match (word >> 10) & 0x3 {
            0 => {
                let v = self.read_f(f) & !(1 << b);
                self.write_f(f, v); // BCF
            }
            1 => {
                let v = self.read_f(f) | (1 << b);
                self.write_f(f, v); // BSF
            }
            2 => {
                if self.read_f(f) & (1 << b) == 0 {
                    return pc + 2; // BTFSC skip if clear
                }
            }
            3 => {
                if self.read_f(f) & (1 << b) != 0 {
                    return pc + 2; // BTFSS skip if set
                }
            }
            _ => unreachable!(),
        }
        pc + 1
    }

    fn exec_call_goto(&mut self, pc: u16, word: u16) -> u16 {
        let k = word & 0x7FF;
        // GOTO/CALL: PC<14:11> from PCLATH<6:3>, the 11-bit literal is
        // PC<10:0> (DS41364E Figure 3-4); PCLATH is not modified.
        let target = ((self.ram[0x0A] as u16 & 0x78) << 8) | k;
        if word & 0x0800 != 0 {
            target // GOTO
        } else {
            self.stack.push(pc + 1); // CALL
            target
        }
    }

    fn exec_literal(&mut self, pc: u16, word: u16) -> u16 {
        let op6 = (word >> 8) & 0x3F;
        let k = (word & 0xFF) as u8;
        match op6 {
            0x35 | 0x36 | 0x37 | 0x3B | 0x3D => {
                // The PIC14E shift/add/sub-with-carry byte ops live in the
                // 11-prefixed family (DS41364E Table 29-3).
                let d = (word >> 7) & 1;
                let f = (word & 0x7F) as usize;
                match op6 {
                    0x3D => {
                        // ADDWFC: f + W + C.
                        let v = self.read_f(f);
                        let cin = (self.ram[3] & 0b001 != 0) as u8;
                        let r = self.w.wrapping_add(v).wrapping_add(cin);
                        self.set_z(r);
                        self.set_c((self.w as u16 + v as u16 + cin as u16) > 0xFF);
                        self.set_dc(
                            ((self.w & 0x0F) as u16 + (v & 0x0F) as u16 + cin as u16) > 0x0F,
                        );
                        self.write_d(d, f, r);
                    }
                    0x3B => {
                        // SUBWFB: f - W - (1 - C); C = 1 when no borrow
                        // (f >= W + (1-C)), the SUBWF polarity.
                        let v = self.read_f(f);
                        let cin = (self.ram[3] & 0b001 != 0) as u8;
                        let bor = 1 - cin;
                        let r = v.wrapping_sub(self.w).wrapping_sub(bor);
                        self.set_z(r);
                        self.set_c(v >= self.w.wrapping_add(bor));
                        self.set_dc((v & 0x0F) >= (self.w & 0x0F).wrapping_add(bor));
                        self.write_d(d, f, r);
                    }
                    0x37 => {
                        // ASRF: arithmetic right shift, MSb held.
                        let v = self.read_f(f);
                        self.set_c(v & 0x01 != 0);
                        let r = (v >> 1) | (v & 0x80);
                        self.set_z(r);
                        self.write_d(d, f, r);
                    }
                    0x35 => {
                        // LSLF: left shift through... LSLF shifts left,
                        // bit 7 into C, bit 0 cleared.
                        let v = self.read_f(f);
                        self.set_c(v & 0x80 != 0);
                        let r = v << 1;
                        self.set_z(r);
                        self.write_d(d, f, r);
                    }
                    _ => {
                        // LSRF: logical right shift, bit 0 into C.
                        let v = self.read_f(f);
                        self.set_c(v & 0x01 != 0);
                        let r = v >> 1;
                        self.set_z(r);
                        self.write_d(d, f, r);
                    }
                }
                return pc + 1;
            }
            0x31 => {
                // MOVLP sets word bit 7 (`11 0001 1kk kkkk`); ADDFSR's 6-bit
                // k leaves it clear (`11 0001 0nkk kkkk`).
                if word & 0x80 != 0 {
                    self.ram[0x0A] = (word & 0x7F) as u8; // MOVLP
                } else {
                    // ADDFSR FSRn, k: signed 6-bit k (DS41364E).
                    let n = ((word >> 6) & 1) as usize;
                    let k = if word & 0x20 != 0 {
                        ((word & 0x3F) as i16) - 64
                    } else {
                        (word & 0x3F) as i16
                    };
                    let cur = self.fsr(n) as i16;
                    self.set_fsr(n, cur.wrapping_add(k) as u16);
                }
                return pc + 1;
            }
            0x32 | 0x33 => {
                // BRA: PC = PC + 1 + signed 9-bit literal.
                let k = word & 0x1FF;
                let off = if k & 0x100 != 0 {
                    (k | 0xFE00) as i16
                } else {
                    k as i16
                };
                return (pc as i16 + 1 + off) as u16;
            }
            0x3F => {
                // Indexed MOVIW/MOVWI: 11 1111 dnkk kkkk, signed 6-bit k.
                let n = ((word >> 6) & 1) as usize;
                let k = if word & 0x20 != 0 {
                    ((word & 0x3F) as i16) - 64
                } else {
                    (word & 0x3F) as i16
                };
                let eff = self.fsr(n).wrapping_add(k as u16);
                if word & 0x80 != 0 {
                    self.write_ram(eff, self.w); // MOVWI
                } else {
                    let v = self.read_ram(eff);
                    self.set_z(v);
                    self.w = v; // MOVIW
                }
                return pc + 1;
            }
            _ => {}
        }
        match (word >> 8) & 0xF {
            0xE | 0xF => {
                let r = self.w.wrapping_add(k);
                self.add_flags(self.w, k, r);
                self.w = r;
            }
            0x9 => {
                self.w &= k;
                self.set_z(self.w);
            }
            0x8 => {
                self.w |= k;
                self.set_z(self.w);
            }
            0xA => {
                self.w ^= k;
                self.set_z(self.w);
            }
            0xC | 0xD => {
                let r = k.wrapping_sub(self.w);
                self.set_z(r);
                self.set_c(k >= self.w);
                self.set_dc((k & 0x0F) >= (self.w & 0x0F));
                self.w = r;
            }
            0x0..=0x3 => self.w = k, // MOVLW
            0x4..=0x7 => {
                self.w = k; // RETLW
                let ret = self.pop_return();
                return ret;
            }
            _ => unreachable!(),
        }
        pc + 1
    }
}

/// PIC12F509 (baseline, 12-bit-word core) instruction-set simulator.
/// The baseline register file (DS41236E Figure 4-4): the SFR block
/// 0x00-0x06 and the shared GPR 0x07-0x0F are bank-independent, bank 0 GPR
/// is 0x10-0x1F and bank 1 GPR 0x30-0x3F. `FSR<5>` selects the bank for
/// *both* direct and indirect addressing (D-2, docs/37): a direct operand
/// `f` resolves to `(FSR<5> << 5) | f`, and `INDF` reads/writes
/// `RAM[FSR & 0x3F]` (the full flat address). The 2-level hardware
/// call/return stack (D-4) is a shift register: CALL pushes PC+1 (level 1
/// to level 2), RETLW pops level 1 into PC and copies level 2 into level 1.
/// No interrupt vector or RETFIE on this core.
pub struct PicBaseline<'a> {
    device: &'a Device,
    prog: Vec<u16>,
    /// The 509's full 6-bit data space (0x00-0x3F).
    ram: [u8; 64],
    w: u8,
    /// 11-bit program counter; PC<9> comes from STATUS PA0, PC<8> is forced
    /// to 0 by every PCL-modifying instruction except GOTO (DS41236E
    /// section 4.7).
    pc: u16,
    /// The 2-deep hardware stack, level 1 at index 0 (DS41236E section 4.8).
    stack: [u16; 2],
    halted: bool,
    /// Write-only shadow registers: TRISGPIO (TRIS f) and OPTION. Neither is
    /// addressable in the file map (DS41236E Table 4-1), so they live
    /// outside `ram`.
    tris: u8,
    option: u8,
}

impl<'a> PicBaseline<'a> {
    /// A simulator on `device`'s memory map. Only the `Core::PicBaseline`
    /// contract is checked; the 509's geometry is the P1 target.
    pub fn with_device(device: &'a Device, prog: Vec<u16>) -> Self {
        assert_eq!(
            device.core,
            device::Core::PicBaseline,
            "sim(pic-baseline): {} is not a pic-baseline device",
            device.name
        );
        PicBaseline {
            device,
            prog,
            ram: [0; 64],
            w: 0,
            pc: 0,
            stack: [0; 2],
            halted: false,
            tris: 0,
            option: 0,
        }
    }
    pub fn ram(&self) -> &[u8; 64] {
        &self.ram
    }
    pub fn ram_mut(&mut self) -> &mut [u8; 64] {
        &mut self.ram
    }
    pub fn w(&self) -> u8 {
        self.w
    }
    pub fn pc(&self) -> u16 {
        self.pc
    }
    pub fn halted(&self) -> bool {
        self.halted
    }
    /// The write-only TRISGPIO shadow register.
    pub fn tris(&self) -> u8 {
        self.tris
    }
    /// The write-only OPTION shadow register.
    pub fn option(&self) -> u8 {
        self.option
    }
    pub fn run(&mut self, max_steps: usize) -> usize {
        let mut steps = 0;
        while !self.halted && steps < max_steps {
            self.step();
            steps += 1;
        }
        steps
    }
    pub fn step(&mut self) {
        let word = self.prog[self.pc as usize];
        let pc = self.pc;
        let next = match (word >> 10) & 0x3 {
            0 => self.exec_byte(pc, word),
            1 => self.exec_bit(pc, word),
            2 => self.exec_call_goto(pc, word),
            3 => self.exec_literal(pc, word),
            _ => unreachable!(),
        };
        self.pc = next;
        if self.pc as usize >= self.prog.len() {
            self.halted = true;
        }
    }

    /// The bank selected by `FSR`'s high bits: `FSR<5>` on the 509, masked
    /// to `fsr_bank_bits` so a wider part (16F505's `FSR<6:5>`) resolves
    /// the same way.
    fn fsr_bank(&self) -> usize {
        let bits = self.device.fsr_bank_bits as usize;
        ((self.ram[0x04] >> 5) & ((1 << bits) - 1)) as usize
    }
    /// The physical address of a direct operand: the SFR block 0x00-0x06
    /// and the shared GPR 0x07-0x0F are bank-independent (DS41236E Figure
    /// 4-4); everything else is paged by `FSR<5>`.
    fn direct_addr(&self, f: usize) -> usize {
        if f <= 0x0F {
            f
        } else {
            (self.fsr_bank() << 5) | f
        }
    }
    /// The physical address `INDF` selects: the full flat `FSR` value
    /// (bank bits and offset together, DS41236E Figure 4-7).
    fn indirect_addr(&self) -> usize {
        (self.ram[0x04] & 0x3F) as usize
    }
    fn read_f(&self, f: usize) -> u8 {
        match f {
            0x00 => self.ram[self.indirect_addr()], // INDF
            0x02 => (self.pc & 0xFF) as u8,         // PCL
            _ => self.ram[self.direct_addr(f)],
        }
    }
    fn write_f(&mut self, f: usize, v: u8) {
        match f {
            0x00 => {
                let a = self.indirect_addr();
                self.ram[a] = v; // INDF
            }
            0x02 => {
                // PCL write: PC<7:0> = v, PC<8> = 0, PC<9> = PA0
                // (DS41236E section 4.7).
                let pa0 = (self.ram[0x03] & 0x20) as u16;
                self.pc = (pa0 << 4) | (v as u16);
            }
            0x03 => {
                // STATUS: TO (bit 4) and PD (bit 3) are read-only; the
                // writable bits (GPWUF, PA0, Z, DC, C) take the write
                // (DS41236E section 4.4).
                self.ram[0x03] = (self.ram[0x03] & 0x18) | (v & 0xE7);
            }
            _ => {
                let a = self.direct_addr(f);
                self.ram[a] = v;
            }
        }
    }
    fn write_d(&mut self, d: u16, f: usize, r: u8) {
        if d == 1 {
            self.write_f(f, r);
        } else {
            self.w = r;
        }
    }
    fn set_z(&mut self, v: u8) {
        if v == 0 {
            self.ram[0x03] |= 0b100;
        } else {
            self.ram[0x03] &= !0b100;
        }
    }
    fn set_c(&mut self, c: bool) {
        if c {
            self.ram[0x03] |= 0b001;
        } else {
            self.ram[0x03] &= !0b001;
        }
    }
    fn set_dc(&mut self, c: bool) {
        if c {
            self.ram[0x03] |= 0b010;
        } else {
            self.ram[0x03] &= !0b010;
        }
    }
    fn add_flags(&mut self, a: u8, b: u8, r: u8) {
        self.set_z(r);
        self.set_c((a as u16 + b as u16) > 0xFF);
        self.set_dc(((a & 0x0F) as u16 + (b & 0x0F) as u16) > 0x0F);
    }
    fn rlf(&mut self, v: u8) -> u8 {
        let cin = if self.ram[0x03] & 0b001 != 0 { 1 } else { 0 };
        let cout = v >> 7;
        let r = (v << 1) | cin;
        self.set_c(cout != 0);
        r
    }
    fn rrf(&mut self, v: u8) -> u8 {
        let cin = if self.ram[0x03] & 0b001 != 0 { 0x80 } else { 0 };
        let cout = v & 1;
        let r = (v >> 1) | cin;
        self.set_c(cout != 0);
        r
    }
    /// CALL pushes PC+1, shifting level 1 to level 2 (DS41236E section 4.8).
    fn stack_push(&mut self, v: u16) {
        self.stack[1] = self.stack[0];
        self.stack[0] = v;
    }
    /// RETLW pops level 1 into the PC and copies level 2 into level 1.
    /// Underflow returns 0 (no status bits, DS41236E section 4.8 note 1).
    fn stack_pop(&mut self) -> u16 {
        let ret = self.stack[0];
        self.stack[0] = self.stack[1];
        ret
    }

    fn exec_byte(&mut self, pc: u16, word: u16) -> u16 {
        match word {
            0x0000 => return pc + 1, // NOP
            0x0002 => {
                self.option = self.w; // OPTION: W -> OPTION shadow
                return pc + 1;
            }
            0x0003 => {
                self.halted = true; // SLEEP
                return pc;
            }
            0x0004 => return pc + 1, // CLRWDT
            0x0040 => {
                self.w = 0; // CLRW
                self.set_z(0);
                return pc + 1;
            }
            _ => {}
        }
        let d = (word >> 5) & 1;
        let f = (word & 0x1F) as usize;
        let op6 = (word >> 6) & 0x3F;
        match op6 {
            0x00 => {
                // MOVWF is `0000 001f ffff` (bit 5 set), TRIS `0000 0000
                // 0fff` (bit 5 clear): the same op6, told apart by bit 5.
                if word & 0x020 != 0 {
                    if f == 0x02 {
                        // MOVWF PCL: the whole PC changes (DS41236E section
                        // 4.7), so this is a control-flow instruction, not a
                        // plain store. write_f set self.pc; return it.
                        self.write_f(f, self.w);
                        return self.pc;
                    }
                    self.write_f(f, self.w); // MOVWF
                } else {
                    self.tris = self.w; // TRIS f
                }
            }
            0x01 => {
                self.write_f(f, 0); // CLRF
                self.set_z(0);
            }
            0x02 => {
                let v = self.read_f(f); // SUBWF
                let r = v.wrapping_sub(self.w);
                self.set_z(r);
                self.set_c(v >= self.w);
                self.set_dc((v & 0x0F) >= (self.w & 0x0F));
                self.write_d(d, f, r);
            }
            0x03 => {
                let r = self.read_f(f).wrapping_sub(1); // DECF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x04 => {
                let r = self.w | self.read_f(f); // IORWF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x05 => {
                let r = self.w & self.read_f(f); // ANDWF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x06 => {
                let r = self.w ^ self.read_f(f); // XORWF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x07 => {
                let v = self.read_f(f); // ADDWF
                let r = self.w.wrapping_add(v);
                self.add_flags(self.w, v, r);
                self.write_d(d, f, r);
            }
            0x08 => {
                let r = self.read_f(f); // MOVF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x09 => {
                let r = !self.read_f(f); // COMF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x0A => {
                let r = self.read_f(f).wrapping_add(1); // INCF
                self.set_z(r);
                self.write_d(d, f, r);
            }
            0x0B => {
                let r = self.read_f(f).wrapping_sub(1); // DECFSZ
                self.write_d(d, f, r);
                if r == 0 {
                    return pc + 2;
                }
            }
            0x0C => {
                let r = self.rrf(self.read_f(f)); // RRF
                self.write_d(d, f, r);
            }
            0x0D => {
                let r = self.rlf(self.read_f(f)); // RLF
                self.write_d(d, f, r);
            }
            0x0E => {
                let v = self.read_f(f); // SWAPF
                let r = (v << 4) | (v >> 4);
                self.write_d(d, f, r);
            }
            0x0F => {
                let r = self.read_f(f).wrapping_add(1); // INCFSZ
                self.write_d(d, f, r);
                if r == 0 {
                    return pc + 2;
                }
            }
            other => panic!("sim(pic-baseline): byte opcode {other:#x} not implemented"),
        }
        pc + 1
    }
    fn exec_bit(&mut self, pc: u16, word: u16) -> u16 {
        let b = ((word >> 5) & 0x7) as u8;
        let f = (word & 0x1F) as usize;
        match (word >> 8) & 0x3 {
            0 => self.write_f(f, self.read_f(f) & !(1 << b)), // BCF
            1 => self.write_f(f, self.read_f(f) | (1 << b)),  // BSF
            2 => {
                if self.read_f(f) & (1 << b) == 0 {
                    return pc + 2; // BTFSC skip if clear
                }
            }
            3 => {
                if self.read_f(f) & (1 << b) != 0 {
                    return pc + 2; // BTFSS skip if set
                }
            }
            _ => unreachable!(),
        }
        pc + 1
    }
    fn exec_call_goto(&mut self, pc: u16, word: u16) -> u16 {
        let k = word & 0x1FF;
        let pa0 = (self.ram[0x03] & 0x20) as u16;
        // The three control ops share top bits `10` (DS41236E Table 8-2):
        // GOTO is `101k` (bit 9 set), CALL `1001` (bit 8 set), RETLW `1000`
        // (neither). GOTO's k is 9 bits; CALL/RETLW's are 8.
        if word & 0x0200 != 0 {
            // GOTO: PC<8:0> = k, PC<9> = PA0.
            (pa0 << 4) | k
        } else if word & 0x0100 != 0 {
            // CALL: PC<7:0> = k, PC<8> = 0, PC<9> = PA0; push PC+1.
            self.stack_push(pc + 1);
            (pa0 << 4) | (k & 0xFF)
        } else {
            // RETLW: W = k, pop the return address.
            self.w = (word & 0xFF) as u8;
            self.stack_pop()
        }
    }
    fn exec_literal(&mut self, pc: u16, word: u16) -> u16 {
        let k = (word & 0xFF) as u8;
        match (word >> 8) & 0xF {
            0xC => self.w = k, // MOVLW
            0xD => {
                self.w |= k; // IORLW
                self.set_z(self.w);
            }
            0xE => {
                self.w &= k; // ANDLW
                self.set_z(self.w);
            }
            0xF => {
                self.w ^= k; // XORLW
                self.set_z(self.w);
            }
            _ => unreachable!(),
        }
        pc + 1
    }
}

/// Decode Intel HEX into 16-bit words for a PIC18F4550-sized program
/// (`0x4000` words = 32768 bytes of flash). Same wire format as
/// `parse_hex` (`asm::to_hex` emits identical HEX regardless of core), just
/// sized for PIC18's larger flash.
pub fn parse_hex_pic18(data: &str) -> Vec<u16> {
    let mut words = vec![0u16; 0x4000];
    // Config words live far above flash (PIC18F4550 config base 0x300000,
    // DS39632E §25.1); their `:04` extended-linear-address records must be
    // tracked so each data record's 16-bit address resolves against the
    // real base, and the config data dropped (it is not flash). Without
    // the tracking, a config record's low-16 address aliases flash word 0.
    let mut extended_upper: usize = 0;
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        assert!(line.starts_with(':'), "not Intel HEX: {line}");
        let bytes = hex_decode(&line[1..]);
        let len = bytes[0] as usize;
        let addr = ((bytes[1] as usize) << 8) | (bytes[2] as usize);
        let rectype = bytes[3];
        let data = &bytes[4..4 + len];
        match rectype {
            0x00 => {
                let base = extended_upper + addr;
                // Config-region records (0x300000, beyond the 0x4000-word
                // flash window) carry no program data; skip them rather
                // than writing into the flash array.
                if base >= 0x8000 {
                    continue;
                }
                for (i, chunk) in data.chunks(2).enumerate() {
                    let w = (chunk[0] as u16) | ((chunk[1] as u16) << 8);
                    words[base / 2 + i] = w;
                }
            }
            0x01 => break,
            0x04 => {
                extended_upper = ((data[0] as usize) << 8 | data[1] as usize) << 16;
            }
            other => panic!("unsupported HEX record type {other:#x}"),
        }
    }
    words
}

/// Decode the driver's PIC14E Intel HEX output into 14-bit words, indexed
/// by word address. Word addressing and dynamic sizing match `parse_hex`,
/// but the ELA handling matches `parse_hex_pic18`: the driver emits
/// CONFIG1/CONFIG2 as a separate region at byte 0x1000E, and a
/// parser that ignores 0x04 records lets that region's low-16 address
/// alias program words.
pub fn parse_hex_pic14e(data: &str) -> Vec<u16> {
    let mut max_word = 8191usize; // Keeps the minimum program size for small images.
    let mut extended_upper: usize = 0;
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let bytes = hex_decode(&line[1..]);
        if bytes[3] == 0x04 {
            extended_upper = ((bytes[4] as usize) << 8 | bytes[5] as usize) << 16;
        }
        if bytes[3] == 0x00 && extended_upper == 0 {
            max_word = max_word
                .max(((bytes[1] as usize) << 8 | bytes[2] as usize) / 2 + bytes[0] as usize / 2);
        }
    }
    let mut words = vec![0u16; max_word + 1];
    extended_upper = 0;
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        assert!(line.starts_with(':'), "not Intel HEX: {line}");
        let bytes = hex_decode(&line[1..]);
        let len = bytes[0] as usize;
        let addr = ((bytes[1] as usize) << 8) | (bytes[2] as usize);
        let rectype = bytes[3];
        let data = &bytes[4..4 + len];
        match rectype {
            0x00 => {
                // Program records sit at base 0; the only other region the
                // driver emits is the config words at 0x10000 and up, and
                // the simulator does not model config bytes.
                if extended_upper != 0 {
                    continue;
                }
                for (i, chunk) in data.chunks(2).enumerate() {
                    let w = (chunk[0] as u16) | ((chunk[1] as u16) << 8);
                    words[addr / 2 + i] = w;
                }
            }
            0x01 => break,
            0x04 => {
                extended_upper = ((data[0] as usize) << 8 | data[1] as usize) << 16;
            }
            other => panic!("unsupported HEX record type {other:#x}"),
        }
    }
    words
}

/// PIC18F4550 (16-bit-word core) instruction-set simulator. `pc` is a
/// **byte** address (PIC18's PC natively counts bytes, incrementing by 2
/// per one-word instruction), unlike `Pic14::pc`, which is a word
/// address. This matches the real hardware and lets the interrupt vectors
/// (0x000008/0x000018) and `GOTO`/`CALL`'s encoded targets be used
/// directly without a unit conversion at every call site.
pub struct Pic18 {
    prog: Vec<u16>,
    ram: [u8; 4096],
    w: u8,
    pc: u32,
    /// Hardware call stack: up to 31 return byte-addresses. `TOSU`/`TOSH`/
    /// `TOSL`/`STKPTR` (SFRs 0xFFF/0xFFE/0xFFD/0xFFC) are computed views
    /// over this, not separate storage: mirrors how `Pic14::read_f`
    /// special-cases the `PCL` SFR address over the `pc` field instead of
    /// storing it twice.
    stack: Vec<u32>,
    halted: bool,
    /// A latched interrupt request awaiting INTCON GIE + INT0IE. Set by
    /// `request_interrupt`, consumed when the interrupt is taken.
    pending: bool,
    /// A latched LOW-priority request awaiting INTCON GIEH + GIEL + INT0IE
    /// (the IPEN=1 routing: firmware owns RCON.IPEN, the sim models the
    /// post-IPEN behavior). Set by `request_low_interrupt`, consumed on
    /// low-vector entry.
    pending_lo: bool,
    /// The live ISR nesting stack, outermost first: `true` = high-priority
    /// context. Pushed on vector entry (both `fire_` hooks and the
    /// request paths), popped by RETFIE to restore the right enable bit.
    isr_stack: Vec<bool>,
    /// A PC<7:0> write (computed jump through `PCL`) waiting to override
    /// this instruction's linear `next`, consumed by `step`'s tail.
    jump_target: Option<u32>,
    /// The data-EEPROM cell array behind the EEADR/EEDATA/EECON1/EECON2
    /// register file.
    eeprom: Eeprom,
}

impl Pic18 {
    pub fn new(prog: Vec<u16>) -> Self {
        Pic18 {
            prog,
            ram: [0; 4096],
            w: 0,
            pc: 0,
            stack: Vec::new(),
            eeprom: Eeprom::new(),
            halted: false,
            pending: false,
            pending_lo: false,
            isr_stack: Vec::new(),
            jump_target: None,
        }
    }
    pub fn ram(&self) -> &[u8; 4096] {
        &self.ram
    }
    pub fn ram_mut(&mut self) -> &mut [u8; 4096] {
        &mut self.ram
    }
    pub fn eeprom(&self) -> &[u8; 256] {
        &self.eeprom.cells
    }
    /// Route a physical store through the data-EEPROM register file
    /// (same contract as `Pic14::ee_store`).
    fn ee_store(&mut self, addr: usize, v: u8) -> Option<u8> {
        if addr != PIC18_EECON1 && addr != PIC18_EECON2 {
            return None;
        }
        let eeadr = self.ram[PIC18_EEADR];
        let wren = self.ram[PIC18_EECON1] & 0x04 != 0;
        self.eeprom.on_store(
            addr,
            PIC18_EECON1,
            PIC18_EECON2,
            v,
            eeadr,
            &mut self.ram[PIC18_EEDATA],
            wren,
        )
    }
    pub fn w(&self) -> u8 {
        self.w
    }
    pub fn pc(&self) -> u32 {
        self.pc
    }
    pub fn halted(&self) -> bool {
        self.halted
    }
    pub fn run(&mut self, max_steps: usize) -> usize {
        let mut steps = 0;
        while !self.halted && steps < max_steps {
            self.step();
            steps += 1;
        }
        steps
    }
    pub fn step(&mut self) {
        // Interrupts are recognised at an instruction boundary: a latched,
        // enabled request vectors instead of executing this instruction,
        // which then runs on return (mirrors `Pic14::step`).
        if self.interrupt_ready() {
            self.pending = false;
            self.enter_isr();
            return; // vectoring costs its own cycle; the handler runs next
        }
        // The low vector is checked second: a pending high request
        // preempts even when a low one is also latched (hardware priority).
        if self.interrupt_ready_lo() {
            self.pending_lo = false;
            self.enter_isr_low();
            return;
        }
        let word = self.prog[(self.pc / 2) as usize];
        let pc = self.pc;
        let next = match word {
            0x0000 => pc + 2,
            0x0003 => {
                // SLEEP: matches Pic14's convention (see `Pic14::exec_byte`)
                // of `halted = true` as the simulator's stop condition:
                // real programs end on this, since `parse_hex_pic18`
                // returns the full flash-sized buffer (zero-padded NOPs
                // all the way out), so running off the end of `prog`
                // stays out of reach for programs within a step budget.
                self.halted = true;
                pc
            }
            0x0004 => pc + 2, // CLRWDT: no observable effect in this simulator
            0x0005 => {
                // PUSH: pushes PC+2 (the address of the next instruction)
                // without jumping: the same stack effect as CALL, minus the
                // jump.
                self.push_return(pc + 2);
                pc + 2
            }
            0x0006 => {
                // POP: discards the top of stack without jumping (unlike
                // RETURN, which also jumps to it).
                self.pop_return();
                pc + 2
            }
            0x0007 => {
                self.exec_daw();
                pc + 2
            }
            0x00FF => {
                // RESET: reinitializes the core. This simulator only
                // models what's observable to a test: PC to 0, W cleared,
                // the call stack emptied (its SFR view synced to match).
                self.w = 0;
                self.stack.clear();
                self.sync_stack_sfrs();
                0
            }
            // MUST precede the byte-oriented arm below: 0x0800..=0x0FFF is
            // numerically inside 0x0200..=0x6FFF.
            _ if (0x0800..=0x0FFF).contains(&word) => self.exec_literal(pc, word),
            _ if (0x0200..=0x6FFF).contains(&word) => self.exec_byte(pc, word),
            _ if (0x7000..=0xBFFF).contains(&word) => self.exec_bit(pc, word),
            _ if (0xE000..=0xE7FF).contains(&word) => self.exec_cond_branch(pc, word),
            _ if (0xD000..=0xDFFF).contains(&word) => self.exec_bra_rcall(pc, word),
            0xC000..=0xCFFF => {
                let w2 = self.prog[(pc / 2) as usize + 1];
                self.exec_movff(pc, word, w2)
            }
            0xEC00..=0xEFFF => {
                let w2 = self.prog[(pc / 2) as usize + 1];
                self.exec_goto_call_lfsr(pc, word, w2)
            }
            0x0010 | 0x0011 => self.exec_retfie(),
            0x0012 | 0x0013 => self.pop_return(),
            // TBLRD* / TBLRD*+ / TBLRD*- / TBLRD+*: single-word opcodes
            // that read one byte of program memory into TABLAT. Must
            // precede the literal arm below (0x0008..0x000B are numerically
            // inside 0x0200..=0x6FFF).
            0x0008 | 0x0009 | 0x000A | 0x000B => {
                self.exec_tblrd(word);
                pc + 2
            }
            0x0100..=0x010F => {
                self.ram[0xFE0] = (word & 0xF) as u8; // MOVLB: loads BSR
                pc + 2
            }
            _ => panic!("sim(pic18): opcode {word:#06x} not yet implemented"),
        };
        // A `MOVWF PCL` (computed jump) sets the whole PC from
        // PCLATU:PCLATH:W; its linear next is void.
        let next = self.jump_target.take().unwrap_or(next);
        self.pc = next;
        if (self.pc / 2) as usize >= self.prog.len() {
            self.halted = true;
        }
    }

    /// The byte address just after a skip instruction's own effect where
    /// execution resumes on a SKIP. Real PIC18 hardware skips an extra word
    /// when the instruction being skipped is a two-word form
    /// (`GOTO`/`CALL`/`LFSR`/`MOVFF`). `after_pc` is the address right
    /// after the skip instruction itself (where the skipped instruction
    /// starts); peek its opcode to decide.
    fn skip_pc(&self, after_pc: u32) -> u32 {
        let word = self.prog[(after_pc / 2) as usize];
        let is_two_word = matches!(word & 0xFF00, 0xEF00 | 0xEE00)
            || word & 0xFE00 == 0xEC00
            || word >> 12 == 0xC;
        after_pc + if is_two_word { 4 } else { 2 }
    }

    /// Byte-oriented dispatch: masks off the variable fields and matches the
    /// fixed "base" bits directly against the encoding table's hex
    /// constants. The two groups carry different-width fixed fields (the
    /// d+a+f group's fixed bits are `word & 0xFC00`; the a+f-only group's
    /// are `word & 0xFE00`, since bit9 belongs to its fixed identifier).
    /// Recovering opcodes by shift-then-narrow-mask arithmetic collides
    /// unrelated opcodes, so the match uses the wide masks directly.
    fn exec_byte(&mut self, pc: u32, word: u16) -> u32 {
        let a = (word >> 8) & 1;
        let d = (word >> 9) & 1;
        let f = word & 0xFF;
        // Resolves the operand once per instruction: a d=1 access on a
        // virtual register applies its side effect once for the whole
        // read-modify-write. Resolving again at write-back repeats the
        // side effect and corrupts the software-stack frame.
        let op = self.resolve_f(a, f);
        // No-destination-select group first (`word & 0xFE00`): CLRF/
        // CPFSEQ/CPFSGT/CPFSLT/MOVWF/MULWF/NEGF/SETF/TSTFSZ.
        match word & 0xFE00 {
            0x6A00 => {
                // CLRF
                self.write_phys(op, 0);
                self.set_z(0);
                return pc + 2;
            }
            0x6200 => {
                // CPFSEQ: skip if f == W, no flags
                if self.read_phys(op) == self.w {
                    return self.skip_pc(pc + 2);
                }
                return pc + 2;
            }
            0x6E00 => {
                // MOVWF: W -> f, no flags
                self.write_phys(op, self.w);
                return pc + 2;
            }
            0x6400 => {
                // CPFSGT: skip if f > W (unsigned), no flags
                if self.read_phys(op) > self.w {
                    return self.skip_pc(pc + 2);
                }
                return pc + 2;
            }
            0x6000 => {
                // CPFSLT: skip if f < W (unsigned), no flags
                if self.read_phys(op) < self.w {
                    return self.skip_pc(pc + 2);
                }
                return pc + 2;
            }
            0x6C00 => {
                // NEGF: f = 0 - f, full flags
                let fv = self.read_phys(op);
                let r = 0u8.wrapping_sub(fv);
                self.sub_flags(0, fv, r);
                self.set_zn(r);
                self.write_phys(op, r);
                return pc + 2;
            }
            0x6800 => {
                // SETF: f = 0xFF, no flags
                self.write_phys(op, 0xFF);
                return pc + 2;
            }
            0x6600 => {
                // TSTFSZ: skip if f == 0, no flags
                if self.read_phys(op) == 0 {
                    return self.skip_pc(pc + 2);
                }
                return pc + 2;
            }
            0x0200 => {
                // MULWF: unsigned 8x8 -> 16-bit product in PRODH:PRODL
                let prod = (self.w as u16) * (self.read_phys(op) as u16);
                self.ram[0xFF3] = (prod & 0xFF) as u8; // PRODL
                self.ram[0xFF4] = (prod >> 8) as u8; // PRODH
                return pc + 2;
            }
            _ => {}
        }
        // Destination-select group (`word & 0xFC00`).
        match word & 0xFC00 {
            0x2400 => {
                // ADDWF: f + W
                let fv = self.read_phys(op);
                let r = fv.wrapping_add(self.w);
                self.add_flags(fv, self.w, r);
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x5C00 => {
                // SUBWF: f - W
                let fv = self.read_phys(op);
                let r = fv.wrapping_sub(self.w);
                self.sub_flags(fv, self.w, r);
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x3800 => {
                // SWAPF: nibble swap, no flags
                let v = self.read_phys(op);
                let r = (v << 4) | (v >> 4);
                self.write_d_at(d, op, r);
            }
            0x2C00 => {
                // DECFSZ
                let r = self.read_phys(op).wrapping_sub(1);
                self.write_d_at(d, op, r);
                if r == 0 {
                    return self.skip_pc(pc + 2);
                }
            }
            0x2000 => {
                // ADDWFC: f + W + C
                let fv = self.read_phys(op);
                let cin = self.get_c() as u8;
                let r = self.addc_flags(fv, self.w, cin);
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x1400 => {
                // ANDWF: f & W
                let r = self.read_phys(op) & self.w;
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x1C00 => {
                // COMF: !f
                let r = !self.read_phys(op);
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x4C00 => {
                // DCFSNZ: f - 1, skip if NOT zero
                let r = self.read_phys(op).wrapping_sub(1);
                self.set_zn(r);
                self.write_d_at(d, op, r);
                if r != 0 {
                    return self.skip_pc(pc + 2);
                }
            }
            0x2800 => {
                // INCF: f + 1
                let fv = self.read_phys(op);
                let r = fv.wrapping_add(1);
                self.add_flags(fv, 1, r);
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x3C00 => {
                // INCFSZ: f + 1, skip if zero
                let fv = self.read_phys(op);
                let r = fv.wrapping_add(1);
                self.write_d_at(d, op, r);
                if r == 0 {
                    return self.skip_pc(pc + 2);
                }
            }
            0x4800 => {
                // INFSNZ: f + 1, skip if NOT zero
                let fv = self.read_phys(op);
                let r = fv.wrapping_add(1);
                self.write_d_at(d, op, r);
                if r != 0 {
                    return self.skip_pc(pc + 2);
                }
            }
            0x1000 => {
                // IORWF: f | W
                let r = self.read_phys(op) | self.w;
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x5000 => {
                // MOVF: f (copy), no ALU op
                let r = self.read_phys(op);
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x3400 => {
                // RLCF: rotate left through C
                let v = self.read_phys(op);
                let cin = self.get_c() as u8;
                let r = (v << 1) | cin;
                self.set_c(v & 0x80 != 0);
                self.write_d_at(d, op, r);
            }
            0x4400 => {
                // RLNCF: rotate left, bit7 wraps to bit0, no carry
                let v = self.read_phys(op);
                let r = v.rotate_left(1);
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x3000 => {
                // RRCF: rotate right through C
                let v = self.read_phys(op);
                let cin = self.get_c() as u8;
                let r = (v >> 1) | (cin << 7);
                self.set_c(v & 0x01 != 0);
                self.write_d_at(d, op, r);
            }
            0x4000 => {
                // RRNCF: rotate right, bit0 wraps to bit7, no carry
                let v = self.read_phys(op);
                let r = v.rotate_right(1);
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x5400 | 0x5800 => {
                // SUBFWB / SUBWFB: f - W - !C, computed as f + !W + C (the
                // ALU adder with W inverted; both mnemonics share this
                // computation).
                let fv = self.read_phys(op);
                let cin = self.get_c() as u8;
                let r = self.addc_flags(fv, !self.w, cin);
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x1800 => {
                // XORWF: f ^ W
                let r = self.read_phys(op) ^ self.w;
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            0x0400 => {
                // DECF: f - 1
                let fv = self.read_phys(op);
                let r = fv.wrapping_sub(1);
                self.sub_flags(fv, 1, r);
                self.set_zn(r);
                self.write_d_at(d, op, r);
            }
            other => panic!(
                "sim(pic18): byte opcode base {other:#06x} (word {word:#06x}) not yet implemented"
            ),
        }
        pc + 2
    }

    fn exec_bit(&mut self, pc: u32, word: u16) -> u32 {
        let a = (word >> 8) & 1;
        let f = word & 0xFF;
        let b = (word >> 9) & 0x7;
        // One resolve for the read-modify-write, same contract as
        // `exec_byte`'s head resolve.
        let op = self.resolve_f(a, f);
        match (word >> 12) & 0xF {
            0x7 => {
                let v = self.read_phys(op);
                self.write_phys(op, v ^ (1 << b)); // BTG
            }
            0x8 => {
                let v = self.read_phys(op);
                self.write_phys(op, v | (1 << b)); // BSF
            }
            0x9 => {
                let v = self.read_phys(op);
                self.write_phys(op, v & !(1 << b)); // BCF
            }
            0xA => {
                if self.read_phys(op) & (1 << b) != 0 {
                    return self.skip_pc(pc + 2); // BTFSS: skip if set
                }
            }
            0xB => {
                if self.read_phys(op) & (1 << b) == 0 {
                    return self.skip_pc(pc + 2); // BTFSC: skip if clear
                }
            }
            other => panic!("sim(pic18): bit opcode group {other:#03x} unreachable"),
        }
        pc + 2
    }

    fn exec_literal(&mut self, pc: u32, word: u16) -> u32 {
        let k = (word & 0xFF) as u8;
        match (word >> 8) & 0xF {
            0x8 => {
                // SUBLW: k - W
                let r = self.addc_flags(k, !self.w, 1);
                self.set_zn(r);
                self.w = r;
            }
            0x9 => {
                self.w |= k; // IORLW
                self.set_zn(self.w);
            }
            0xA => {
                self.w ^= k; // XORLW
                self.set_zn(self.w);
            }
            0xB => {
                self.w &= k; // ANDLW
                self.set_zn(self.w);
            }
            0xC => {
                // RETLW: W = k, then return
                self.w = k;
                return self.pop_return();
            }
            0xD => {
                // MULLW: unsigned 8x8 -> 16-bit product in PRODH:PRODL
                let prod = (self.w as u16) * (k as u16);
                self.ram[0xFF3] = (prod & 0xFF) as u8; // PRODL
                self.ram[0xFF4] = (prod >> 8) as u8; // PRODH
            }
            0xE => self.w = k, // MOVLW
            0xF => {
                // ADDLW: W + k
                let r = self.addc_flags(self.w, k, 0);
                self.set_zn(r);
                self.w = r;
            }
            other => unreachable!("literal opcode nibble {other:#x}"),
        }
        pc + 2
    }
    /// `TOSU`/`TOSH`/`TOSL`/`STKPTR` (`0xFFF`/`0xFFE`/`0xFFD`/`0xFFC`) are
    /// real physical SFRs on hardware (not just a simulator convenience),
    /// so `push_return`/`pop_return` keep them in sync in `self.ram` on
    /// every call: `self.stack` is only the internal push/pop mechanism.
    fn sync_stack_sfrs(&mut self) {
        self.ram[0xFFC] = self.stack.len() as u8;
        let top = self.stack.last().copied().unwrap_or(0);
        self.ram[0xFFD] = (top & 0xFF) as u8;
        self.ram[0xFFE] = ((top >> 8) & 0xFF) as u8;
        self.ram[0xFFF] = ((top >> 16) & 0xFF) as u8;
    }
    /// Decimal-adjusts W after a BCD addition: if the low nibble exceeds 9
    /// or DC is set, adds 6; if the adjusted high nibble exceeds 9 or C
    /// is set, adds 0x60 and sets C (DAW sets C without clearing it,
    /// matching the datasheet sticky carry-out convention).
    fn exec_daw(&mut self) {
        let dc = self.ram[self.status_addr()] & 0x02 != 0;
        let mut w = self.w;
        if (w & 0x0F) > 9 || dc {
            w = w.wrapping_add(6);
        }
        let c = self.get_c();
        let mut carry = c;
        if (w & 0xF0) > 0x90 || c {
            w = w.wrapping_add(0x60);
            carry = true;
        }
        self.w = w;
        self.set_c(carry);
    }

    /// The 21-bit byte address `TBLPTRU:TBLPTRH:TBLPTRL` points at
    /// (`TBLPTRL` = `0xFF6`, `TBLPTRH` = `0xFF7`, `TBLPTRU` = `0xFF8`).
    fn tblptr(&self) -> u32 {
        ((self.ram[0xFF8] as u32) << 16) | ((self.ram[0xFF7] as u32) << 8) | self.ram[0xFF6] as u32
    }
    fn set_tblptr(&mut self, v: u32) {
        self.ram[0xFF6] = (v & 0xFF) as u8;
        self.ram[0xFF7] = ((v >> 8) & 0xFF) as u8;
        self.ram[0xFF8] = ((v >> 16) & 0xFF) as u8;
    }
    /// The program-memory byte at byte address `addr` in the byte-packed
    /// flash model (`asm`'s `DB` packing): even address = word low byte,
    /// odd address = word high byte. Reads past the end of `prog` as 0x00.
    fn read_flash_byte(&self, addr: u32) -> u8 {
        let w = self.prog.get((addr / 2) as usize).copied().unwrap_or(0);
        if addr % 2 == 0 {
            (w & 0xFF) as u8
        } else {
            (w >> 8) as u8
        }
    }
    /// Execute one of the four `TBLRD*` forms: read the byte at `TBLPTR`
    /// into `TABLAT` (SFR `0xFF5`), then apply the pointer's side effect
    /// (`*`: none; `*+`: post-increment; `*-`: post-decrement; `+*`:
    /// pre-increment). TBLPTR is a BYTE address, so the step is 1, not 2;
    /// the 21-bit register wraps mod `0x200000` exactly like the hardware.
    fn exec_tblrd(&mut self, opcode: u16) {
        match opcode {
            0x000B => {
                // TBLRD+*: increment BEFORE the read.
                self.set_tblptr(self.tblptr().wrapping_add(1) & 0x1F_FFFF);
            }
            _ => {}
        }
        self.ram[0xFF5] = self.read_flash_byte(self.tblptr());
        match opcode {
            0x0009 => self.set_tblptr(self.tblptr().wrapping_add(1) & 0x1F_FFFF), // TBLRD*+
            0x000A => self.set_tblptr(self.tblptr().wrapping_sub(1) & 0x1F_FFFF), // TBLRD*-
            _ => {}
        }
    }

    fn push_return(&mut self, addr: u32) {
        assert!(
            self.stack.len() < 31,
            "sim(pic18): call stack overflow (depth 31)"
        );
        self.stack.push(addr);
        self.sync_stack_sfrs();
    }
    fn pop_return(&mut self) -> u32 {
        let addr = self.stack.pop().unwrap_or(0);
        self.sync_stack_sfrs();
        addr
    }

    fn exec_cond_branch(&mut self, pc: u32, word: u16) -> u32 {
        let n = (word & 0xFF) as i8 as i32;
        let status = self.ram[self.status_addr()];
        let taken = match (word >> 8) & 0x7 {
            0 => status & 0x04 != 0, // BZ: Z set
            1 => status & 0x04 == 0, // BNZ
            2 => status & 0x01 != 0, // BC
            3 => status & 0x01 == 0, // BNC
            4 => status & 0x08 != 0, // BOV
            5 => status & 0x08 == 0, // BNOV
            6 => status & 0x10 != 0, // BN
            7 => status & 0x10 == 0, // BNN
            _ => unreachable!(),
        };
        if taken {
            let next_word = (pc / 2) as i32 + 1 + n;
            (next_word as u32) * 2
        } else {
            pc + 2
        }
    }

    fn exec_bra_rcall(&mut self, pc: u32, word: u16) -> u32 {
        let raw = word & 0x7FF;
        let n = if raw & 0x400 != 0 {
            (raw as i32) - 0x800
        } else {
            raw as i32
        }; // sign-extend 11 bits
        let is_call = word & 0x0800 != 0;
        let next_word = (pc / 2) as i32 + 1 + n;
        if is_call {
            self.push_return(pc + 2);
        }
        (next_word as u32) * 2
    }

    fn exec_goto_call_lfsr(&mut self, pc: u32, word: u16, word2: u16) -> u32 {
        let k12 = (word2 & 0xFFF) as u32;
        match word & 0xFF00 {
            0xEF00 => {
                let k = (k12 << 8) | (word & 0xFF) as u32;
                k * 2 // word address -> byte address
            }
            0xEC00 | 0xED00 => {
                let k = (k12 << 8) | (word & 0xFF) as u32;
                self.push_return(pc + 4);
                k * 2
            }
            0xEE00 => {
                let fsr = ((word >> 4) & 0x3) as usize;
                let k = ((word & 0xF) as u16) << 8 | (word2 & 0xFF);
                let (lo_addr, hi_addr) = match fsr {
                    0 => (0xFE9, 0xFEA),
                    1 => (0xFE1, 0xFE2),
                    2 => (0xFD9, 0xFDA),
                    _ => unreachable!(),
                };
                self.ram[lo_addr] = (k & 0xFF) as u8;
                self.ram[hi_addr] = (k >> 8) as u8;
                pc + 4
            }
            _ => panic!("sim(pic18): unrecognized two-word opcode {word:#06x}"),
        }
    }

    fn exec_movff(&mut self, pc: u32, word: u16, word2: u16) -> u32 {
        // MOVFF's two 12-bit operands already name full physical addresses:
        // MOVFF bypasses the `a` bit and `BSR` and addresses the whole
        // linear data space directly, so they resolve through
        // `resolve_phys`, not `resolve_f`. `resolve_f` re-derives a physical
        // address from an assumed 8-bit field, which double-adds `0xF00`
        // inside the SFR page and leaves banked GPR above 0x5F out of range.
        //
        // Resolves `src` and reads its value before resolving `dst`: a
        // `POSTINCn`-to-`POSTINCm` copy advances both FSRs exactly once
        // each, since `resolve_phys` performs the post-increment while
        // resolving the address, not as a separate step.
        let src = self.resolve_phys((word & 0xFFF) as usize);
        let val = self.read_phys(src);
        let dst = self.resolve_phys((word2 & 0xFFF) as usize);
        self.write_phys(dst, val);
        pc + 4
    }

    /// RETFIE restores the preempted context's enable bit (INTCON bit 7
    /// GIEH for a high return, bit 6 GIEL for a low one: the hardware's
    /// shadow restore) and pops the return address, resuming the
    /// interrupted code. An empty nesting stack (a hand-written program
    /// that RETFIEs without a modelled entry) keeps the historical
    /// behavior: GIEH back on. (The interrupt-entry modelling that clears
    /// GIE landed with P5's interrupt model; before that RETFIE behaved
    /// like RETURN.)
    fn exec_retfie(&mut self) -> u32 {
        match self.isr_stack.pop() {
            Some(true) => self.ram[0xFF2] |= 0x80,  // high return: GIEH on
            Some(false) => self.ram[0xFF2] |= 0x40, // low return: GIEL on
            None => self.ram[0xFF2] |= 0x80,        // unmodelled: GIE on
        }
        self.pop_return()
    }

    fn status_addr(&mut self) -> usize {
        self.resolve_f(0, 0xD8)
    }
    fn set_z(&mut self, v: u8) {
        let addr = self.status_addr();
        if v == 0 {
            self.ram[addr] |= 0x04
        } else {
            self.ram[addr] &= !0x04
        }
    }
    fn set_c(&mut self, c: bool) {
        let addr = self.status_addr();
        if c {
            self.ram[addr] |= 0x01
        } else {
            self.ram[addr] &= !0x01
        }
    }
    fn set_dc(&mut self, c: bool) {
        let addr = self.status_addr();
        if c {
            self.ram[addr] |= 0x02
        } else {
            self.ram[addr] &= !0x02
        }
    }
    fn set_n(&mut self, r: u8) {
        let addr = self.status_addr();
        if r & 0x80 != 0 {
            self.ram[addr] |= 0x10
        } else {
            self.ram[addr] &= !0x10
        }
    }
    fn set_ov(&mut self, ov: bool) {
        let addr = self.status_addr();
        if ov {
            self.ram[addr] |= 0x08
        } else {
            self.ram[addr] &= !0x08
        }
    }
    /// Z, N always follow the result; call after every byte-oriented op.
    fn set_zn(&mut self, r: u8) {
        self.set_z(r);
        self.set_n(r);
    }
    /// Add flags (C/DC/OV) for `a + b = r` (used by ADDWF/ADDWFC/ADDLW).
    fn add_flags(&mut self, a: u8, b: u8, r: u8) {
        self.set_c((a as u16 + b as u16) > 0xFF);
        self.set_dc(((a & 0x0F) as u16 + (b & 0x0F) as u16) > 0x0F);
        self.set_ov(((a ^ r) & (b ^ r) & 0x80) != 0);
    }
    fn get_c(&mut self) -> bool {
        self.ram[self.status_addr()] & 0x01 != 0
    }
    /// `a + b + cin`, setting C/DC/OV for the 3-operand add and returning
    /// the wrapped result. Used by ADDWFC and (with `b` inverted) by
    /// SUBFWB/SUBWFB, which PIC18's ALU computes as an add-with-carry.
    fn addc_flags(&mut self, a: u8, b: u8, cin: u8) -> u8 {
        let sum: u16 = a as u16 + b as u16 + cin as u16;
        let r = sum as u8;
        self.set_c(sum > 0xFF);
        let dc_sum: u16 = (a & 0x0F) as u16 + (b & 0x0F) as u16 + cin as u16;
        self.set_dc(dc_sum > 0x0F);
        self.set_ov(((a ^ r) & (b ^ r) & 0x80) != 0);
        r
    }
    /// Subtract flags (C/DC/OV) for `a - b = r` (PIC "no borrow" convention:
    /// C=1 means a>=b, i.e. no borrow). Used by SUBWF/SUBLW/CPFS*/DECF etc.
    fn sub_flags(&mut self, a: u8, b: u8, r: u8) {
        self.set_c(a >= b);
        self.set_dc((a & 0x0F) >= (b & 0x0F));
        self.set_ov(((a ^ b) & (a ^ r) & 0x80) != 0);
    }

    /// Resolves a byte/bit-oriented `(a, f)` pair to its physical 12-bit
    /// address. `a=0` (access bank): `f<=0x5F` maps to `f` (low access,
    /// `0x000-0x05F`); `f>0x5F` maps to `0xF00+f` (high access/SFR,
    /// `0xF60-0xFFF`). `a=1` (banked) maps to `(BSR<<8)|f`, hard-coded
    /// here exactly as `Pic14::bank_base` hard-codes RP1:RP0.
    ///
    /// Checks indirect registers (`INDFn`/`POSTINCn`/`POSTDECn`/
    /// `PREINCn`/`PLUSWn`) against the resolved RESULT at `0xFD9-0xFEF`,
    /// rather than against the raw `f` byte in isolation. A `BSR`-banked GPR
    /// low byte coincidentally matches e.g. `0xE7` (INDF1) while its address
    /// lands outside the SFR page; matching raw `f` misroutes such writes
    /// into FSR side effects and panics the array index on a negative
    /// `PLUSWn` offset cast. Only the resolved address decides.
    fn resolve_f(&mut self, a: u16, f: u16) -> usize {
        let phys = if a == 0 {
            if f <= 0x5F {
                f as usize
            } else {
                0xF00 + f as usize
            }
        } else {
            ((self.ram[0xFE0] as usize) << 8) | f as usize
        };
        self.resolve_phys(phys)
    }

    /// Shared back half of indirect-address resolution: given a
    /// fully-formed physical address (`resolve_f`'s `phys`, or an
    /// `exec_movff` operand, which already names a physical address with
    /// no `a`/`BSR` reconstruction), detects whether it lands on one of
    /// the `INDFn`/`POSTINCn`/`POSTDECn`/`PREINCn`/`PLUSWn`
    /// pseudo-registers and dereferences through the named `FSRn`
    /// (applying the increment/decrement side effect where applicable).
    /// Otherwise `phys` is already the answer.
    ///
    /// Matches on `phys & 0xFF` alone, as `resolve_f` does, rather than on
    /// a separately-threaded raw register-file byte, for the reason above:
    /// a `BSR`-banked GPR low byte coincidentally equals e.g. `0xE7`
    /// (INDF1) while its physical address lands outside the SFR page, so
    /// only the resolved in-page `phys` decides indirect identity.
    fn resolve_phys(&mut self, phys: usize) -> usize {
        if phys < 0xF00 {
            return phys;
        }
        let (fsrn_lo, fsrn_hi, indf, postinc, postdec, preinc) = match (phys & 0xFF) as u16 {
            0xEF | 0xEE | 0xED | 0xEC | 0xEB => (0xE9, 0xEA, 0xEF, 0xEE, 0xED, 0xEC), // FSR0
            0xE7 | 0xE6 | 0xE5 | 0xE4 | 0xE3 => (0xE1, 0xE2, 0xE7, 0xE6, 0xE5, 0xE4), // FSR1
            0xDF | 0xDE | 0xDD | 0xDC | 0xDB => (0xD9, 0xDA, 0xDF, 0xDE, 0xDD, 0xDC), // FSR2
            _ => return phys,
        };
        let lo_addr = 0xF00 + fsrn_lo;
        let hi_addr = 0xF00 + fsrn_hi;
        let cur = ((self.ram[hi_addr] as u16) << 8) | self.ram[lo_addr] as u16;
        let f = (phys & 0xFF) as u16;
        match f {
            _ if f == indf => cur as usize,
            _ if f == postinc => {
                let next = cur.wrapping_add(1);
                self.ram[lo_addr] = (next & 0xFF) as u8;
                self.ram[hi_addr] = (next >> 8) as u8;
                cur as usize
            }
            _ if f == postdec => {
                let next = cur.wrapping_sub(1);
                self.ram[lo_addr] = (next & 0xFF) as u8;
                self.ram[hi_addr] = (next >> 8) as u8;
                cur as usize
            }
            _ if f == preinc => {
                let next = cur.wrapping_add(1);
                self.ram[lo_addr] = (next & 0xFF) as u8;
                self.ram[hi_addr] = (next >> 8) as u8;
                next as usize
            }
            _ => {
                // PLUSWn: cur + (signed W), no side effect on FSRn.
                let offset = self.w as i8 as i32;
                ((cur as i32) + offset) as usize
            }
        }
    }
    /// WREG is the access-bank file register 0xFE8 (DS39632E table 5-1),
    /// not separate storage: an instruction reading `f = 0xFE8` reads W,
    /// and a write there sets W. Without this routing, chained `RLNCF
    /// WREG, W` / `SWAPF WREG, W` sequences read the flat RAM's
    /// always-zero 0xFE8 byte and every W-chained computation collapses
    /// to zero.
    fn read_phys(&mut self, addr: usize) -> u8 {
        match addr {
            // WREG is the access-bank file register 0xFE8 (DS39632E
            // table 5-1), not separate storage: an instruction reading
            // `f = 0xFE8` reads W, and a write there sets W. Without this
            // routing, chained `RLNCF WREG, W` / `SWAPF WREG, W`
            // sequences read the flat RAM's always-zero 0xFE8 byte and
            // every W-chained computation collapses to zero.
            0xFE8 => self.w,
            // PCL reads as the PC's low byte (DS39632E section 4.3).
            0xFF9 => (self.pc & 0xFF) as u8,
            _ => self.ram[addr],
        }
    }
    fn write_phys(&mut self, addr: usize, v: u8) {
        match addr {
            0xFE8 => self.w = v,
            // Writing PCL jumps: PC = PCLATU:PCLATH:v, the computed-call
            // mechanism SDCC's `__sdcc_call` helpers use. The jump is
            // latched because `step` assigns the linear next after the
            // dispatch returns.
            0xFF9 => {
                let pclath = self.ram[0xFFA] as u32;
                let pclatu = (self.ram[0xFFB] as u32) & 0x1F;
                self.jump_target = Some((pclatu << 16) | (pclath << 8) | v as u32);
            }
            // TOSL/TOSH/TOSU are the hardware stack's top-of-stack view
            // (`sync_stack_sfrs` mirrors the other direction): SDCC plants
            // a computed-call return address by PUSHing, then overwriting
            // the TOS registers, so the writes must land in the stack.
            0xFFD | 0xFFE | 0xFFF => {
                self.ram[addr] = v;
                if let Some(top) = self.stack.last_mut() {
                    let shift = 8 * (addr - 0xFFD);
                    *top = (*top & !(0xFFu32 << shift)) | ((v as u32) << shift);
                }
            }
            _ => {
                let v = self.ee_store(addr, v).unwrap_or(v);
                self.ram[addr] = v;
            }
        }
    }
    fn write_d_at(&mut self, d: u16, op: usize, r: u8) {
        if d == 1 {
            self.write_phys(op, r);
        } else {
            self.w = r;
        }
    }

    /// Fire the high-priority interrupt immediately, bypassing INTCON
    /// gating: push the return address and jump to vector 0x0008. The
    /// unconditional test hook, mirroring `Pic14::fire_interrupt`: use it
    /// to place an interrupt at an exact program counter without modelling
    /// INTCON. Called BETWEEN steps, so `pc` addresses an instruction that
    /// still awaits execution; the return address is `pc` itself and RETFIE
    /// resumes by running it.
    pub fn fire_interrupt(&mut self) {
        self.enter_isr();
    }
    /// Request the interrupt through the modelled path: latch it and set
    /// INT0IF (INTCON bit 1). It is taken at the next step boundary at
    /// which INTCON bits 7 (GIE) and 4 (INT0IE) are both set, so a program
    /// that masks interrupts keeps it pending until it unmasks. The latch
    /// is consumed on entry, so a handler that leaves INT0IF set still
    /// runs once rather than looping. (PIC18 INTCON = 0xFF2, same bit
    /// layout as PIC14's.)
    pub fn request_interrupt(&mut self) {
        self.ram[0xFF2] |= 0x02; // INT0IF
        self.pending = true;
    }
    /// Whether a requested interrupt remains latched and untaken.
    pub fn interrupt_pending(&self) -> bool {
        self.pending
    }
    /// Fire the low-priority interrupt immediately, bypassing INTCON
    /// gating: push the return address and jump to vector 0x0018. The
    /// unconditional low test hook, mirroring `fire_interrupt`. Only
    /// GIEL is cleared on entry (GIEH stays set), so a high request can
    /// still preempt the low handler, exactly like hardware.
    pub fn fire_low_interrupt(&mut self) {
        self.enter_isr_low();
    }
    /// Request the low-priority interrupt through the modelled path:
    /// latch it and set TMR0IF (INTCON bit 2) as the observable source
    /// flag. It is taken at the next step boundary at which INTCON bits
    /// 7 (GIEH), 6 (GIEL) and 4 (the modelled source enable) are all
    /// set. The latch is consumed on entry, mirroring `request_interrupt`.
    pub fn request_low_interrupt(&mut self) {
        self.ram[0xFF2] |= 0x04; // TMR0IF
        self.pending_lo = true;
    }
    /// Whether a requested low-priority interrupt is still latched.
    pub fn low_interrupt_pending(&self) -> bool {
        self.pending_lo
    }
    /// Push the return address, clear GIE (INTCON bit 7: hardware does
    /// this on entry so the handler is not immediately re-entered) and
    /// vector to 0x0008.
    fn enter_isr(&mut self) {
        self.stack.push(self.pc);
        self.ram[0xFF2] &= !0x80; // clear GIE
        self.pc = 0x0008;
        self.isr_stack.push(true);
    }
    /// Push the return address, clear GIEL only (INTCON bit 6) and vector
    /// to 0x0018. GIEH is left set: hardware keeps high interrupts
    /// enabled inside a low handler so they can preempt it.
    fn enter_isr_low(&mut self) {
        self.stack.push(self.pc);
        self.ram[0xFF2] &= !0x40; // clear GIEL
        self.pc = 0x0018;
        self.isr_stack.push(false);
    }
    /// A latched request whose global and source enables are both set.
    fn interrupt_ready(&self) -> bool {
        self.pending && self.ram[0xFF2] & 0x80 != 0 && self.ram[0xFF2] & 0x10 != 0
    }
    /// A latched low request whose master, low-global and source enables
    /// are all set (GIEH gates everything when IPEN = 1).
    fn interrupt_ready_lo(&self) -> bool {
        self.pending_lo
            && self.ram[0xFF2] & 0x80 != 0
            && self.ram[0xFF2] & 0x40 != 0
            && self.ram[0xFF2] & 0x10 != 0
    }
}

#[cfg(test)]
mod pic18_interrupt {
    use super::Pic18;

    /// A program with NOPs at bytes 0/2/4 (words 0-2) and an ISR at the
    /// high vector: word 4 (byte 8) = `MOVWF 0x20,A`, word 5 (byte 10) =
    /// `RETFIE`.
    fn pic_with_isr() -> Pic18 {
        let mut prog = vec![0u16; 16];
        prog[0] = 0x0000; // NOP at byte 0
        prog[1] = 0x0000; // NOP at byte 2
        prog[2] = 0x0000; // NOP at byte 4
                          // ISR at byte 8 (word 4): MOVWF 0x020,A then RETFIE
        prog[4] = 0x6E20; // MOVWF 0x20,A
        prog[5] = 0x0010; // RETFIE
        Pic18::new(prog)
    }

    #[test]
    fn fire_interrupt_vectors_to_0x0008_and_retfie_resumes() {
        let mut pic = pic_with_isr();
        pic.run(2); // two NOPs, pc == 4
        assert_eq!(pic.pc(), 4);
        pic.w = 0x2A; // the preempted main's W
        pic.fire_interrupt();
        assert_eq!(pic.pc(), 0x0008, "fire must vector to 0x0008");
        assert_eq!(pic.ram()[0xFF2] & 0x80, 0, "GIE cleared on entry");
        pic.step(); // the ISR's MOVWF 0x20,A (byte 8 -> pc 10)
        pic.step(); // RETFIE (byte 10 -> pops byte 4)
        assert_eq!(pic.ram[0x20], 0x2A, "ISR stored W to 0x20");
        assert_eq!(pic.pc(), 4, "RETFIE resumes the interrupted instruction");
        assert_eq!(pic.ram[0xFF2] & 0x80, 0x80, "GIE re-enabled on RETFIE");
    }

    #[test]
    fn request_interrupt_is_gated_by_gie_and_int0ie() {
        let mut pic = pic_with_isr();
        pic.ram[0xFF2] = 0x10; // INT0IE only, GIE clear
        pic.request_interrupt();
        assert!(pic.interrupt_pending(), "requested interrupt must latch");
        pic.run(3); // three NOPs (bytes 0,2,4), pc == 6
        assert_eq!(pic.pc(), 6, "must stay masked while GIE is clear");
        pic.ram[0xFF2] = 0x90; // GIE | INT0IE
        pic.run(1); // the boundary check vectors on the next step
        assert_eq!(pic.pc(), 0x0008, "must vector once GIE goes up");
        assert!(!pic.interrupt_pending(), "the latch is consumed on entry");
    }

    /// A program with NOPs at bytes 0/2/4 and ISRs at both vectors: word
    /// 4 (byte 8) = `MOVWF 0x20,A` + `RETFIE`, word 12 (byte 0x18) =
    /// `MOVWF 0x21,A` + `RETFIE`.
    fn pic_with_both_isrs() -> Pic18 {
        let mut prog = vec![0u16; 16];
        prog[0] = 0x0000; // NOP at byte 0
        prog[1] = 0x0000; // NOP at byte 2
        prog[2] = 0x0000; // NOP at byte 4
        prog[4] = 0x6E20; // high ISR at byte 8: MOVWF 0x20,A
        prog[5] = 0x0010; // RETFIE
        prog[12] = 0x6E21; // low ISR at byte 0x18: MOVWF 0x21,A
        prog[13] = 0x0010; // RETFIE
        Pic18::new(prog)
    }

    #[test]
    fn fire_low_vectors_to_0x0018_and_keeps_gieh() {
        let mut pic = pic_with_both_isrs();
        pic.ram[0xFF2] = 0xD0; // GIEH | GIEL | INT0IE
        pic.run(2); // two NOPs, pc == 4
        pic.w = 0x3C; // the preempted main's W
        pic.fire_low_interrupt();
        assert_eq!(pic.pc(), 0x0018, "low fire must vector to 0x0018");
        assert_eq!(pic.ram[0xFF2] & 0x40, 0, "GIEL cleared on low entry");
        assert_eq!(
            pic.ram[0xFF2] & 0x80,
            0x80,
            "GIEH stays set: high can still preempt"
        );
        pic.step(); // the low ISR's MOVWF 0x21,A
        pic.step(); // RETFIE (pops byte 4)
        assert_eq!(pic.ram[0x21], 0x3C, "low ISR stored W to 0x21");
        assert_eq!(pic.pc(), 4, "RETFIE resumes the interrupted instruction");
        assert_eq!(pic.ram[0xFF2] & 0x40, 0x40, "GIEL re-enabled on RETFIE");
    }

    #[test]
    fn high_preempts_low_and_both_restore() {
        let mut pic = pic_with_both_isrs();
        pic.ram[0xFF2] = 0xD0; // GIEH | GIEL | INT0IE
        pic.run(2); // pc == 4
        pic.w = 0x11;
        pic.fire_low_interrupt(); // -> low ISR at 0x18
        pic.step(); // low MOVWF 0x21,A (pc now 0x1A)
        pic.w = 0x22;
        pic.fire_interrupt(); // high preempts the low handler
        assert_eq!(pic.pc(), 0x0008, "high fire vectors to 0x0008");
        assert_eq!(pic.ram[0xFF2] & 0x80, 0, "GIEH cleared on high entry");
        pic.step(); // high MOVWF 0x20,A
        assert_eq!(pic.ram[0x20], 0x22, "high ISR stored W to 0x20");
        pic.step(); // high RETFIE: back into the low handler at 0x1A
        assert_eq!(pic.pc(), 0x001A, "high RETFIE resumes the low handler");
        assert_eq!(pic.ram[0xFF2] & 0x80, 0x80, "GIEH back on after high");
        assert_eq!(
            pic.ram[0xFF2] & 0x40,
            0,
            "GIEL still clear: the low context is live"
        );
        pic.step(); // low RETFIE: back to main at byte 4
        assert_eq!(pic.pc(), 4, "low RETFIE resumes main");
        assert_eq!(pic.ram[0xFF2] & 0x40, 0x40, "GIEL back on after low");
        assert_eq!(pic.ram[0x21], 0x11, "low ISR's store survived preemption");
    }

    #[test]
    fn low_request_needs_gieh_giel_and_source_enable() {
        let mut pic = pic_with_both_isrs();
        pic.ram[0xFF2] = 0x80; // GIEH only: GIEL clear
        pic.request_low_interrupt();
        assert!(pic.low_interrupt_pending(), "low request must latch");
        pic.run(3);
        assert_eq!(pic.pc(), 6, "must stay masked while GIEL is clear");
        pic.ram[0xFF2] = 0xD0; // GIEH | GIEL | INT0IE
        pic.run(1);
        assert_eq!(pic.pc(), 0x0018, "must vector to 0x0018 once enabled");
        assert!(!pic.low_interrupt_pending(), "the latch is consumed");
    }

    #[test]
    fn high_wins_when_both_requests_are_latched() {
        let mut pic = pic_with_both_isrs();
        pic.ram[0xFF2] = 0xD0; // GIEH | GIEL | INT0IE
        pic.request_interrupt();
        pic.request_low_interrupt();
        pic.run(1); // the boundary serves the high request first
        assert_eq!(pic.pc(), 0x0008, "high wins over a latched low request");
    }
}

#[cfg(test)]
mod pic18_tblrd {
    use super::Pic18;

    /// Build a `Pic18` whose flash word at byte 4 is `0x552A` (low byte
    /// 0x2A, high byte 0x55) and whose program starts with `TBLRD*` then
    /// `SLEEP`, with `TBLPTR` preset to byte address 4.
    fn pic_with_table_at_byte4() -> Pic18 {
        let mut prog = vec![0u16; 4];
        prog[0] = 0x0008; // TBLRD*
        prog[1] = 0x0003; // SLEEP
        prog[2] = 0x552A; // table word at byte address 4
        let mut pic = Pic18::new(prog);
        pic.ram_mut()[0xFF6] = 0x04; // TBLPTRL
        pic.ram_mut()[0xFF7] = 0x00; // TBLPTRH
        pic.ram_mut()[0xFF8] = 0x00; // TBLPTRU
        pic
    }

    #[test]
    fn tblrd_star_reads_flash_byte_into_tablat() {
        let mut pic = pic_with_table_at_byte4();
        pic.run(10);
        assert_eq!(pic.ram()[0xFF5], 0x2A, "TABLAT gets the even (low) byte");
        assert_eq!(pic.ram()[0xFF6], 0x04, "TBLRD* leaves TBLPTR unchanged");
    }

    #[test]
    fn tblrd_star_plus_increments_tblptr() {
        let mut pic = pic_with_table_at_byte4();
        pic.prog[0] = 0x0009; // TBLRD*+
        pic.run(10);
        assert_eq!(pic.ram()[0xFF5], 0x2A, "TABLAT = the even byte at 4");
        assert_eq!(pic.ram()[0xFF6], 0x05, "TBLRD*+ advances TBLPTR to 5");
    }

    #[test]
    fn tblrd_star_plus_at_odd_byte_reads_the_high_byte() {
        let mut pic = pic_with_table_at_byte4();
        pic.ram_mut()[0xFF6] = 0x05; // odd byte: high half of word 2
        pic.prog[0] = 0x0009; // TBLRD*+
        pic.run(10);
        assert_eq!(
            pic.ram()[0xFF5],
            0x55,
            "odd TBLPTR reads the word's high byte"
        );
    }

    #[test]
    fn tblrd_star_minus_decrements_tblptr() {
        let mut pic = pic_with_table_at_byte4();
        pic.ram_mut()[0xFF6] = 0x05;
        pic.prog[0] = 0x000A; // TBLRD*-
        pic.run(10);
        assert_eq!(pic.ram()[0xFF5], 0x55, "TABLAT = the byte at 5");
        assert_eq!(pic.ram()[0xFF6], 0x04, "TBLRD*- backs TBLPTR to 4");
    }

    #[test]
    fn tblrd_plus_star_preincrements_tblptr() {
        let mut pic = pic_with_table_at_byte4();
        pic.prog[0] = 0x000B; // TBLRD+*
        pic.run(10);
        assert_eq!(
            pic.ram()[0xFF5],
            0x55,
            "TBLRD+* reads the byte at 5 (pre-incremented)"
        );
        assert_eq!(pic.ram()[0xFF6], 0x05, "TBLRD+* leaves TBLPTR at 5");
    }
}

#[cfg(test)]
mod parse_hex_pic18_extended {
    use super::parse_hex_pic18;

    #[test]
    fn config_region_records_do_not_aliases_flash_word_zero() {
        // An EPIC_CONFIG build emits a `:04` extended-linear-address
        // record for the config region (PIC18F4550 config base 0x300000)
        // followed by config data records at low-16 address 0x0000.
        // Without tracking the extended upper, those data records alias
        // flash word 0 and clobber the reset vector. Checksums are the
        // Intel HEX 2's-complement byte sum.
        let hex = "\
:020000040030CA\n\
:04000000FFFFFFFF\n\
:00000001FF\n";
        let words = parse_hex_pic18(hex);
        assert_eq!(words[0], 0x0000, "flash word 0 must stay untouched");
        assert_eq!(words.len(), 0x4000);
    }

    #[test]
    fn program_region_records_still_land() {
        // Program records at linear address 0 (the common case) are
        // unaffected by the extended-address tracking.
        let hex = "\
:020000040000FA\n\
:040000001122334452\n\
:00000001FF\n";
        let words = parse_hex_pic18(hex);
        assert_eq!(words[0], 0x2211, "little-endian pair at word 0");
        assert_eq!(words[1], 0x4433, "little-endian pair at word 1");
    }
}

#[cfg(test)]
mod pic14_eeprom {
    use super::*;

    /// Write 0x5C to cell 0x10 through the full unlock sequence (WREN,
    /// 0x55 then 0xAA to EECON2, WR), then read it back through RD and
    /// copy it to RAM (DS39582C chapter 4).
    #[test]
    fn write_then_read_round_trips() {
        let prog = vec![
            0x305C, // MOVLW 0x5C
            0x1703, // BSF STATUS, RP1 (bank 2)
            0x008C, // MOVWF EEDATA
            0x3010, // MOVLW 0x10
            0x008D, // MOVWF EEADR
            0x1683, // BSF STATUS, RP0 (bank 3)
            0x3004, // MOVLW 0x04
            0x008C, // MOVWF EECON1 (WREN)
            0x3055, // MOVLW 0x55
            0x008D, // MOVWF EECON2
            0x30AA, // MOVLW 0xAA
            0x008D, // MOVWF EECON2 (armed)
            0x148C, // BSF EECON1, WR (commit)
            0x140C, // BSF EECON1, RD (latch)
            0x1283, // BCF STATUS, RP0 (bank 2)
            0x080C, // MOVF EEDATA, W
            0x1303, // BCF STATUS, RP1 (bank 0)
            0x00A0, // MOVWF 0x20
            0x0063, // SLEEP
        ];
        let mut pic = Pic14::new(prog);
        pic.run(1000);
        assert!(pic.halted());
        assert_eq!(pic.eeprom()[0x10], 0x5C);
        assert_eq!(pic.eeprom()[0x11], 0xFF, "unwritten cells stay erased");
        assert_eq!(pic.ram()[0x20], 0x5C, "RD latches the cell into EEDATA");
    }
}

#[cfg(test)]
mod pic14e_eeprom {
    use super::*;

    /// The same cycle on the Enhanced core's register file (DS41364E):
    /// EEADRL 0x191, EEDATL 0x193, EECON1 0x195, EECON2 0x196, bank 3
    /// selected through BSR.
    #[test]
    fn write_then_read_round_trips() {
        let prog = vec![
            0x305C, // MOVLW 0x5C
            0x0023, // MOVLB 3 (the EE register file lives at 0x19x)
            0x0093, // MOVWF EEDATL
            0x3010, // MOVLW 0x10
            0x0091, // MOVWF EEADRL
            0x3004, // MOVLW 0x04
            0x0095, // MOVWF EECON1 (WREN)
            0x3055, // MOVLW 0x55
            0x0096, // MOVWF EECON2
            0x30AA, // MOVLW 0xAA
            0x0096, // MOVWF EECON2 (armed)
            0x1495, // BSF EECON1, WR (commit)
            0x3000, // MOVLW 0x00
            0x0093, // MOVWF EEDATL (clobber; only RD restores the cell)
            0x1415, // BSF EECON1, RD (latch the cell into EEDATL)
            0x0813, // MOVF EEDATL, W
            0x0020, // MOVLB 0
            0x00A0, // MOVWF 0x20
            0x0063, // SLEEP
        ];
        let mut pic = Pic14e::with_device(&device::PIC16F1938, prog);
        pic.run(1000);
        assert!(pic.halted());
        assert_eq!(pic.eeprom()[0x10], 0x5C);
        assert_eq!(pic.ram()[0x20], 0x5C, "RD latches the cell into EEDATL");
    }
}

#[cfg(test)]
mod pic18_eeprom {
    use super::*;

    /// The same cycle on PIC18, register file per the SDCC 18f4550.h
    /// non-free header: EECON1 0xFA6, EECON2 0xFA7, EEDATA 0xFA8, EEADR
    /// 0xFA9, banked stores with BSR 15.
    #[test]
    fn write_then_read_round_trips() {
        let prog = vec![
            0x0E5C, // MOVLW 0x5C
            0x010F, // MOVLB 15
            0x6FA8, // MOVWF EEDATA, a
            0x0E10, // MOVLW 0x10
            0x6FA9, // MOVWF EEADR, a
            0x0E04, // MOVLW 0x04
            0x6FA6, // MOVWF EECON1, a (WREN)
            0x0E55, // MOVLW 0x55
            0x6FA7, // MOVWF EECON2, a
            0x0EAA, // MOVLW 0xAA
            0x6FA7, // MOVWF EECON2, a (armed)
            0x83A6, // BSF EECON1, WR, a (commit)
            0x81A6, // BSF EECON1, RD, a (latch)
            0x50A8, // MOVF EEDATA, W, a (still banked on BSR 15)
            0x0100, // MOVLB 0
            0x6F20, // MOVWF 0x20, a
            0x0003, // SLEEP
        ];
        let mut pic = Pic18::new(prog);
        pic.run(1000);
        assert!(pic.halted());
        assert_eq!(pic.eeprom()[0x10], 0x5C);
        assert_eq!(pic.ram()[0x20], 0x5C, "RD latches the cell into EEDATA");
    }
}
