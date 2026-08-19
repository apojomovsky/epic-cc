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
        let pc = self.pc;
        let next = match word {
            0x0000 => pc + 2,
            0x0003 => {
                // SLEEP: matches Pic14's convention (see `Pic14::exec_byte`)
                // of `halted = true` as the simulator's stop condition —
                // real programs end on this, since `parse_hex_pic18`
                // returns the full flash-sized buffer (zero-padded NOPs
                // all the way out), so "ran off the end of `prog`" never
                // happens for a realistic program within a normal step
                // budget.
                self.halted = true;
                pc
            }
            0x0004 => pc + 2, // CLRWDT: no observable effect in this simulator
            0x0005 => {
                // PUSH: pushes PC+2 (the address of the next instruction)
                // without jumping — same stack effect as CALL, minus the
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
            0x0100..=0x010F => {
                self.ram[0xFE0] = (word & 0xF) as u8; // MOVLB: loads BSR
                pc + 2
            }
            _ => panic!("sim(pic18): opcode {word:#06x} not yet implemented"),
        };
        self.pc = next;
        if (self.pc / 2) as usize >= self.prog.len() {
            self.halted = true;
        }
    }

    /// The byte address just after a skip instruction's own effect where
    /// execution resumes on a SKIP. Real PIC18 hardware skips an extra word
    /// when the instruction being skipped is a two-word form
    /// (`GOTO`/`CALL`/`LFSR`/`MOVFF`) — `after_pc` is the address right
    /// after the skip instruction itself (where the skipped instruction
    /// starts); peek its opcode to decide.
    fn skip_pc(&self, after_pc: u32) -> u32 {
        let word = self.prog[(after_pc / 2) as usize];
        let is_two_word = matches!(word & 0xFF00, 0xEF00 | 0xEE00)
            || word & 0xFE00 == 0xEC00
            || word >> 12 == 0xC;
        after_pc + if is_two_word { 4 } else { 2 }
    }

    /// Byte-oriented dispatch: mask off the variable fields and match the
    /// fixed "base" bits directly against the encoding table's hex
    /// constants — do not recover an opcode via shift-then-narrow-mask
    /// arithmetic; the two groups have different-width fixed fields (the
    /// d+a+f group's fixed bits are `word & 0xFC00`, clearing d=bit9/
    /// a=bit8/f=bits7-0; the a+f-only group's fixed bits are
    /// `word & 0xFE00`, clearing only a=bit8/f, because bit9 is part of
    /// ITS fixed identifier, not a variable field) — a narrower mask
    /// silently collides unrelated opcodes.
    fn exec_byte(&mut self, pc: u32, word: u16) -> u32 {
        let a = (word >> 8) & 1;
        let d = (word >> 9) & 1;
        let f = word & 0xFF;
        // No-destination-select group first (`word & 0xFE00`): CLRF/
        // CPFSEQ/CPFSGT/CPFSLT/MOVWF/MULWF/NEGF/SETF/TSTFSZ.
        match word & 0xFE00 {
            0x6A00 => {
                // CLRF
                self.write_f(a, f, 0);
                self.set_z(0);
                return pc + 2;
            }
            0x6200 => {
                // CPFSEQ: skip if f == W, no flags
                if self.read_f(a, f) == self.w {
                    return self.skip_pc(pc + 2);
                }
                return pc + 2;
            }
            0x6E00 => {
                // MOVWF: W -> f, no flags
                self.write_f(a, f, self.w);
                return pc + 2;
            }
            0x6400 => {
                // CPFSGT: skip if f > W (unsigned), no flags
                if self.read_f(a, f) > self.w {
                    return self.skip_pc(pc + 2);
                }
                return pc + 2;
            }
            0x6000 => {
                // CPFSLT: skip if f < W (unsigned), no flags
                if self.read_f(a, f) < self.w {
                    return self.skip_pc(pc + 2);
                }
                return pc + 2;
            }
            0x6C00 => {
                // NEGF: f = 0 - f, full flags
                let fv = self.read_f(a, f);
                let r = 0u8.wrapping_sub(fv);
                self.sub_flags(0, fv, r);
                self.set_zn(r);
                self.write_f(a, f, r);
                return pc + 2;
            }
            0x6800 => {
                // SETF: f = 0xFF, no flags
                self.write_f(a, f, 0xFF);
                return pc + 2;
            }
            0x6600 => {
                // TSTFSZ: skip if f == 0, no flags
                if self.read_f(a, f) == 0 {
                    return self.skip_pc(pc + 2);
                }
                return pc + 2;
            }
            0x0200 => {
                // MULWF: unsigned 8x8 -> 16-bit product in PRODH:PRODL
                let prod = (self.w as u16) * (self.read_f(a, f) as u16);
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
                let fv = self.read_f(a, f);
                let r = fv.wrapping_add(self.w);
                self.add_flags(fv, self.w, r);
                self.set_zn(r);
                self.write_d(d, a, f, r);
            }
            0x5C00 => {
                // SUBWF: f - W
                let fv = self.read_f(a, f);
                let r = fv.wrapping_sub(self.w);
                self.sub_flags(fv, self.w, r);
                self.set_zn(r);
                self.write_d(d, a, f, r);
            }
            0x3800 => {
                // SWAPF: nibble swap, no flags
                let v = self.read_f(a, f);
                let r = (v << 4) | (v >> 4);
                self.write_d(d, a, f, r);
            }
            0x2C00 => {
                // DECFSZ
                let r = self.read_f(a, f).wrapping_sub(1);
                self.write_d(d, a, f, r);
                if r == 0 {
                    return self.skip_pc(pc + 2);
                }
            }
            0x2000 => {
                // ADDWFC: f + W + C
                let fv = self.read_f(a, f);
                let cin = self.get_c() as u8;
                let r = self.addc_flags(fv, self.w, cin);
                self.set_zn(r);
                self.write_d(d, a, f, r);
            }
            0x1400 => {
                // ANDWF: f & W
                let r = self.read_f(a, f) & self.w;
                self.set_zn(r);
                self.write_d(d, a, f, r);
            }
            0x1C00 => {
                // COMF: !f
                let r = !self.read_f(a, f);
                self.set_zn(r);
                self.write_d(d, a, f, r);
            }
            0x4C00 => {
                // DCFSNZ: f - 1, skip if NOT zero
                let r = self.read_f(a, f).wrapping_sub(1);
                self.set_zn(r);
                self.write_d(d, a, f, r);
                if r != 0 {
                    return self.skip_pc(pc + 2);
                }
            }
            0x2800 => {
                // INCF: f + 1
                let fv = self.read_f(a, f);
                let r = fv.wrapping_add(1);
                self.add_flags(fv, 1, r);
                self.set_zn(r);
                self.write_d(d, a, f, r);
            }
            0x3C00 => {
                // INCFSZ: f + 1, skip if zero
                let fv = self.read_f(a, f);
                let r = fv.wrapping_add(1);
                self.write_d(d, a, f, r);
                if r == 0 {
                    return self.skip_pc(pc + 2);
                }
            }
            0x4800 => {
                // INFSNZ: f + 1, skip if NOT zero
                let fv = self.read_f(a, f);
                let r = fv.wrapping_add(1);
                self.write_d(d, a, f, r);
                if r != 0 {
                    return self.skip_pc(pc + 2);
                }
            }
            0x1000 => {
                // IORWF: f | W
                let r = self.read_f(a, f) | self.w;
                self.set_zn(r);
                self.write_d(d, a, f, r);
            }
            0x5000 => {
                // MOVF: f (copy), no ALU op
                let r = self.read_f(a, f);
                self.set_zn(r);
                self.write_d(d, a, f, r);
            }
            0x3400 => {
                // RLCF: rotate left through C
                let v = self.read_f(a, f);
                let cin = self.get_c() as u8;
                let r = (v << 1) | cin;
                self.set_c(v & 0x80 != 0);
                self.write_d(d, a, f, r);
            }
            0x4400 => {
                // RLNCF: rotate left, bit7 wraps to bit0, no carry
                let v = self.read_f(a, f);
                let r = v.rotate_left(1);
                self.set_zn(r);
                self.write_d(d, a, f, r);
            }
            0x3000 => {
                // RRCF: rotate right through C
                let v = self.read_f(a, f);
                let cin = self.get_c() as u8;
                let r = (v >> 1) | (cin << 7);
                self.set_c(v & 0x01 != 0);
                self.write_d(d, a, f, r);
            }
            0x4000 => {
                // RRNCF: rotate right, bit0 wraps to bit7, no carry
                let v = self.read_f(a, f);
                let r = v.rotate_right(1);
                self.set_zn(r);
                self.write_d(d, a, f, r);
            }
            0x5400 | 0x5800 => {
                // SUBFWB / SUBWFB: f - W - !C, computed as f + !W + C (the
                // ALU adder with W inverted — see the plan's note that
                // these two mnemonics share this exact computation; no
                // empirical evidence distinguishes them, so both use it).
                let fv = self.read_f(a, f);
                let cin = self.get_c() as u8;
                let r = self.addc_flags(fv, !self.w, cin);
                self.set_zn(r);
                self.write_d(d, a, f, r);
            }
            0x1800 => {
                // XORWF: f ^ W
                let r = self.read_f(a, f) ^ self.w;
                self.set_zn(r);
                self.write_d(d, a, f, r);
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
        match (word >> 12) & 0xF {
            0x7 => {
                let v = self.read_f(a, f);
                self.write_f(a, f, v ^ (1 << b)); // BTG
            }
            0x8 => {
                let v = self.read_f(a, f);
                self.write_f(a, f, v | (1 << b)); // BSF
            }
            0x9 => {
                let v = self.read_f(a, f);
                self.write_f(a, f, v & !(1 << b)); // BCF
            }
            0xA => {
                if self.read_f(a, f) & (1 << b) != 0 {
                    return self.skip_pc(pc + 2); // BTFSS: skip if set
                }
            }
            0xB => {
                if self.read_f(a, f) & (1 << b) == 0 {
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
    /// every call — `self.stack` is only the internal push/pop mechanism.
    fn sync_stack_sfrs(&mut self) {
        self.ram[0xFFC] = self.stack.len() as u8;
        let top = self.stack.last().copied().unwrap_or(0);
        self.ram[0xFFD] = (top & 0xFF) as u8;
        self.ram[0xFFE] = ((top >> 8) & 0xFF) as u8;
        self.ram[0xFFF] = ((top >> 16) & 0xFF) as u8;
    }
    /// Decimal-adjust W after a BCD addition: if the low nibble is > 9 or
    /// DC is set, add 6; if the (possibly-adjusted) high nibble is > 9 or C
    /// is set, add 0x60 and set C (C is only ever set by DAW, never
    /// cleared, matching the datasheet's "sticky" carry-out convention).
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
    fn push_return(&mut self, addr: u32) {
        assert!(self.stack.len() < 31, "sim(pic18): call stack overflow (depth 31)");
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
        let n = if raw & 0x400 != 0 { (raw as i32) - 0x800 } else { raw as i32 }; // sign-extend 11 bits
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
        let src = (word & 0xFFF) as usize;
        let dst = (word2 & 0xFFF) as usize;
        self.ram[dst] = self.ram[src];
        pc + 4
    }

    /// RETFIE also restores GIE/GIEH from the shadow saved on interrupt
    /// entry — no interrupt-entry modelling exists yet in this plan (P1
    /// has no ISR support requirement), so for now RETFIE behaves like
    /// RETURN. Revisit when interrupt modelling is added for PIC18.
    fn exec_retfie(&mut self) -> u32 {
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

    /// Resolve a byte/bit-oriented `(a, f)` pair to its physical 12-bit
    /// address. `a=0` (access bank): `f<=0x5F` -> `f` (low access,
    /// `0x000-0x05F`); `f>0x5F` -> `0xF00+f` (high access/SFR,
    /// `0xF60-0xFFF`). `a=1` (banked): `(BSR<<8)|f`. This split is a core
    /// PIC18 architecture invariant (see the plan's reference section),
    /// hard-coded here exactly as `Pic14::bank_base` hard-codes RP1:RP0.
    ///
    /// Indirect addressing registers (`INDFn`/`POSTINCn`/`POSTDECn`/
    /// `PREINCn`/`PLUSWn`) are checked AFTER the physical address above is
    /// resolved, by matching the RESULT against the SFR addresses those
    /// registers actually live at (`0xFD9-0xFEF`) — never against the raw
    /// `f` byte in isolation. `f`'s low byte alone is ambiguous: a
    /// `BSR`-banked (`a=1`) ordinary GPR access can have a low byte that
    /// coincidentally equals e.g. `0xE1` (FSR1L) while its real physical
    /// address (`BSR<<8 | f`) lands nowhere near the SFR page — matching on
    /// raw `f` treated every such GPR write as an indirect-register access
    /// instead, corrupting unrelated FSRs and, once `cur`/`W` combined into
    /// a negative `PLUSWn` offset, produced an `i32`-to-`usize` cast so
    /// large it panicked `read_f`/`write_f`'s array index outright. Found
    /// via the PIC18 P2 `banked.c` acceptance fixture (Task 15): 90+
    /// `BSR`-banked globals meant some landed at a physical address whose
    /// low byte fell in this range purely by chance.
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
    fn read_f(&mut self, a: u16, f: u16) -> u8 {
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
