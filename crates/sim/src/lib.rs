//! PIC16F877A (14-bit core) instruction-set simulator.
//! Owned, deterministic, cycle-counting, embeddable in `cargo test`.

/// Decode Intel HEX (gpasm output) into 14-bit words, indexed by word address.
pub fn parse_hex(data: &str) -> Vec<u16> {
    let mut words = vec![0u16; 8192];
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

pub struct Pic14 {
    prog: Vec<u16>,
    ram: [u8; 512],
    w: u8,
    pc: u16,
    stack: Vec<u16>,
    halted: bool,
    /// A latched interrupt request awaiting GIE + INTE. Set by
    /// `request_interrupt`, consumed when the interrupt is taken.
    pending: bool,
}

impl Pic14 {
    pub fn new(prog: Vec<u16>) -> Self {
        Pic14 {
            prog,
            ram: [0; 512],
            w: 0,
            pc: 0,
            stack: Vec::new(),
            halted: false,
            pending: false,
        }
    }
    pub fn ram(&self) -> &[u8; 512] {
        &self.ram
    }
    pub fn ram_mut(&mut self) -> &mut [u8; 512] {
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
    /// Fire the F877A's single interrupt immediately, bypassing GIE and the
    /// enable bits: push the return address and jump to the vector. The
    /// unconditional test hook — use it to place an interrupt at an exact
    /// program counter without modelling INTCON.
    ///
    /// `fire_interrupt` is called BETWEEN steps, so `pc` addresses an
    /// instruction that has not executed yet: the return address is `pc`
    /// itself, and RETFIE resumes by running it. (Pushing `pc + 1` would
    /// silently drop that instruction.)
    pub fn fire_interrupt(&mut self) {
        self.enter_isr();
    }
    /// Request the interrupt through the modelled path: latch it and set
    /// INTF. It is taken at the next step boundary at which GIE and INTE are
    /// both set, so a program that masks interrupts keeps it pending until
    /// it unmasks. The latch is consumed on entry, so a handler that never
    /// clears INTF still runs once rather than looping.
    pub fn request_interrupt(&mut self) {
        self.ram[INTCON] |= INTF;
        self.pending = true;
    }
    /// Whether a requested interrupt is still latched and not yet taken.
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
    // Bank base for direct operands in 0x20-0x6F: bank = STATUS<7:5> (RP1:RP0).
    fn bank_base(&self) -> usize {
        ((self.ram[3] >> 5) & 0x3) as usize * 0x80
    }
    fn read_f(&self, f: usize) -> u8 {
        match f {
            0x00 => self.ram[self.indirect_addr()], // INDF -> RAM[FSR] via IRP
            0x02 => (self.pc & 0xFF) as u8,         // PCL
            0x20..=0x6F => self.ram[f + self.bank_base()],
            _ => self.ram[f], // SFR 0x01-0x1F (bank-independent) and common 0x70-0x7F
        }
    }
    fn write_f(&mut self, f: usize, v: u8) {
        match f {
            0x00 => {
                let addr = self.indirect_addr();
                self.ram[addr] = v; // INDF -> RAM[FSR] via IRP
            }
            0x20..=0x6F => self.ram[f + self.bank_base()] = v,
            _ => self.ram[f] = v, // SFR 0x01-0x1F (bank-independent) and common 0x70-0x7F
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
            0x0000 => return pc + 1, // NOP
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

/// Decode Intel HEX into 16-bit words for a PIC18F4550-sized program
/// (`0x4000` words = 32768 bytes of flash). Same wire format as
/// `parse_hex` (`asm::to_hex` emits identical HEX regardless of core), just
/// sized for PIC18's larger flash.
pub fn parse_hex_pic18(data: &str) -> Vec<u16> {
    let mut words = vec![0u16; 0x4000];
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

/// PIC18F4550 (16-bit-word core) instruction-set simulator. `pc` is a
/// **byte** address (PIC18's PC natively counts bytes, incrementing by 2
/// per one-word instruction), unlike `Pic14::pc` which is a word address —
/// this matches the real hardware and lets the interrupt vectors
/// (0x000008/0x000018) and `GOTO`/`CALL`'s encoded targets be used
/// directly without a unit conversion at every call site.
pub struct Pic18 {
    prog: Vec<u16>,
    ram: [u8; 4096],
    w: u8,
    pc: u32,
    /// Hardware call stack: up to 31 return byte-addresses. `TOSU`/`TOSH`/
    /// `TOSL`/`STKPTR` (SFRs 0xFFF/0xFFE/0xFFD/0xFFC) are computed views
    /// over this, not separate storage — mirrors how `Pic14::read_f`
    /// special-cases the `PCL` SFR address over the `pc` field instead of
    /// storing it twice.
    stack: Vec<u32>,
    halted: bool,
}

impl Pic18 {
    pub fn new(prog: Vec<u16>) -> Self {
        Pic18 { prog, ram: [0; 4096], w: 0, pc: 0, stack: Vec::new(), halted: false }
    }
    pub fn ram(&self) -> &[u8; 4096] {
        &self.ram
    }
    pub fn ram_mut(&mut self) -> &mut [u8; 4096] {
        &mut self.ram
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
        let word = self.prog[(self.pc / 2) as usize];
        // Every recognized opcode is added task-by-task from here; a
        // program of just NOPs already exercises the fetch/pc-advance
        // loop end-to-end.
        if word == 0x0000 {
            self.pc += 2;
        } else {
            panic!("sim(pic18): opcode {word:#06x} not yet implemented");
        }
        if (self.pc / 2) as usize >= self.prog.len() {
            self.halted = true;
        }
    }

    /// Resolve a byte/bit-oriented `(a, f)` pair to its physical 12-bit
    /// address. `a=0` (access bank): `f<=0x5F` -> `f` (low access,
    /// `0x000-0x05F`); `f>0x5F` -> `0xF00+f` (high access/SFR,
    /// `0xF60-0xFFF`). `a=1` (banked): `(BSR<<8)|f`. This split is a core
    /// PIC18 architecture invariant (see the plan's reference section),
    /// hard-coded here exactly as `Pic14::bank_base` hard-codes RP1:RP0.
    fn resolve_f(&self, a: u16, f: u16) -> usize {
        if a == 0 {
            if f <= 0x5F {
                f as usize
            } else {
                0xF00 + f as usize
            }
        } else {
            ((self.ram[0xFE0] as usize) << 8) | f as usize
        }
    }
    fn read_f(&self, a: u16, f: u16) -> u8 {
        self.ram[self.resolve_f(a, f)]
    }
    fn write_f(&mut self, a: u16, f: u16, v: u8) {
        let addr = self.resolve_f(a, f);
        self.ram[addr] = v;
    }
    fn write_d(&mut self, d: u16, a: u16, f: u16, r: u8) {
        if d == 1 {
            self.write_f(a, f, r);
        } else {
            self.w = r;
        }
    }
}
