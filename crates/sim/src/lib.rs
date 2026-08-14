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

pub struct Pic14 {
    prog: Vec<u16>,
    ram: [u8; 256],
    w: u8,
    pc: u16,
    stack: Vec<u16>,
    halted: bool,
}

impl Pic14 {
    pub fn new(prog: Vec<u16>) -> Self {
        Pic14 { prog, ram: [0; 256], w: 0, pc: 0, stack: Vec::new(), halted: false }
    }
    pub fn ram(&self) -> &[u8; 256] {
        &self.ram
    }
    pub fn ram_mut(&mut self) -> &mut [u8; 256] {
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
    fn read_f(&self, f: usize) -> u8 {
        self.ram[f] // Task 5 adds INDF/PCL aliasing
    }
    fn write_f(&mut self, f: usize, v: u8) {
        self.ram[f] = v; // Task 5 adds INDF/PCL aliasing
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
    fn pop_return(&mut self) -> u16 {
        self.stack.pop().unwrap_or(0)
    }

    fn exec_byte(&mut self, pc: u16, word: u16) -> u16 {
        match word {
            0x0000 => return pc + 1, // NOP
            0x0008 => return self.pop_return(), // RETURN
            0x0009 => return self.pop_return(), // RETFIE
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
            other => panic!("byte opcode {other:#x} not yet implemented"),
        }
        pc + 1
    }
    fn exec_bit(&mut self, pc: u16, _word: u16) -> u16 {
        pc + 1 // Task 4
    }
    fn exec_call_goto(&mut self, pc: u16, _word: u16) -> u16 {
        pc + 1 // Task 4
    }
    fn exec_literal(&mut self, pc: u16, _word: u16) -> u16 {
        pc + 1 // Task 4
    }
}
