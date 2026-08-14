# Phase 1 — Verification Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the PIC14 instruction-set simulator and its cross-check harness — the oracle that every later phase is validated against — before any compiler code exists.

**Architecture:** A Rust library crate `pic14-sim` exposing a deterministic, cycle-counting PIC16F877A interpreter (35 instructions, 14-bit words, bank-0 + INDF/FSR/PCL/PCLATH semantics) and an Intel HEX decoder. A cross-check integration test assembles hand-written `.asm` with `gpasm` and runs the resulting HEX in our simulator, so our semantics are validated against a third-party assembler from the first commit.

**Tech Stack:** Rust 1.97.1 (workspace + `cargo test`), `gpasm` 1.5.2 (external process, test-only oracle).

**Spec:** [`docs/12-backend-design.md`](../12-backend-design.md) §3 (verification harness) and §4 phase 1.

## Global Constraints

- Build with `nix develop --command cargo …`; never `apt install` toolchain deps (flake pins rustc 1.97.1, gpasm 1.5.2).
- Conventional commits, single line, ≤ 3 lines.
- `gpasm`/`gpsim` are GPL: invoke only as external processes, never link.
- No external assembler/linker in the product; the simulator is our own.
- Each pipeline stage is a crate with a text boundary; the simulator is infrastructure (`crates/sim`), not a stage.
- New files must be `git add`ed before `nix develop` sees them.

---

### Task 1: Cargo workspace and `pic14-sim` crate scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `crates/sim/Cargo.toml`
- Create: `crates/sim/src/lib.rs`

**Interfaces:**
- Produces: a `pic14-sim` library crate that later tasks populate. No public API yet.

- [ ] **Step 1: Write the workspace manifest**

`Cargo.toml` (repo root):

```toml
[workspace]
resolver = "2"
members = ["crates/sim"]
```

- [ ] **Step 2: Write the crate manifest**

`crates/sim/Cargo.toml`:

```toml
[package]
name = "pic14-sim"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
```

- [ ] **Step 3: Write an empty module**

`crates/sim/src/lib.rs`:

```rust
//! PIC16F877A (14-bit core) instruction-set simulator.
//! Owned, deterministic, cycle-counting, embeddable in `cargo test`.
```

- [ ] **Step 4: Verify it builds**

Run: `nix develop --command cargo build`
Expected: exit 0, compiles the empty crate.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/sim/Cargo.toml crates/sim/src/lib.rs
git commit -m "build: scaffold cargo workspace and pic14-sim crate"
```

---

### Task 2: Intel HEX decoder

**Files:**
- Modify: `crates/sim/src/lib.rs`
- Test: `crates/sim/tests/hex.rs`

**Interfaces:**
- Produces: `pub fn parse_hex(data: &str) -> Vec<u16>` — decodes gpasm's Intel HEX (type-00 data records; two little-endian bytes per 14-bit word at byte address `word*2`; type-01 EOF; type-04 extended-linear-address ignored).

- [ ] **Step 1: Write the failing test**

`crates/sim/tests/hex.rs`:

```rust
use pic14_sim::parse_hex;

#[test]
fn decodes_little_endian_words() {
    // goto 0x005 -> 0x2805 -> bytes 05 28 ; movlw 0xAB -> 0x30AB -> AB 30
    let hex = ":020000040000FA\n:040000000528AB30E9\n:00000001FF\n";
    let words = parse_hex(hex);
    assert_eq!(words[0], 0x2805);
    assert_eq!(words[1], 0x30AB);
    assert_eq!(words[2], 0x0000);
    assert_eq!(words[3], 0x0000);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `nix develop --command cargo test --test hex`
Expected: FAIL, `parse_hex` not found.

- [ ] **Step 3: Implement the decoder**

Append to `crates/sim/src/lib.rs`:

```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `nix develop --command cargo test --test hex`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/sim/src/lib.rs crates/sim/tests/hex.rs
git commit -m "feat(sim): decode Intel HEX into 14-bit words"
```

---

### Task 3: Simulator state, run loop, and byte-oriented instructions

**Files:**
- Modify: `crates/sim/src/lib.rs`
- Test: `crates/sim/tests/byte_ops.rs`

**Interfaces:**
- Produces:
  - `pub struct Pic14 { … }` with `pub fn new(prog: Vec<u16>) -> Pic14`, `pub fn step(&mut self)`, `pub fn run(&mut self, max_steps: usize) -> usize`, `pub fn ram(&self) -> &[u8; 256]`, `pub fn ram_mut(&mut self) -> &mut [u8; 256]`, `pub fn w(&self) -> u8`, `pub fn pc(&self) -> u16`, `pub fn halted(&self) -> bool`.
  - `fn read_f(&self, f: usize) -> u8`, `fn write_f(&mut self, f: usize, v: u8)`, `fn write_d(&mut self, d: u16, f: usize, r: u8)` — INDF/FSR and PCL aliasing live here (Task 5 fills in the special cases; Task 3 wires the plumbing).

- [ ] **Step 1: Write the failing test**

`crates/sim/tests/byte_ops.rs`:

```rust
use pic14_sim::Pic14;

fn run(words: &[u16], init_w: u8) -> Pic14 {
    let mut p = Pic14::new(words.to_vec());
    p.ram_mut()[0x20] = init_w; // set a data value at 0x20
    p.run(1000);
    p
}

#[test]
fn movwf_then_movf_roundtrip() {
    // MOVLW 0x2A ; MOVWF 0x20 ; MOVF 0x20,W ; MOVWF 0x21
    let p = run(&[0x302A, 0x00A0, 0x0820, 0x00A1], 0);
    assert_eq!(p.ram()[0x21], 0x2A);
}

#[test]
fn addwf_carries_and_zero() {
    // MOVLW 0xFF ; MOVWF 0x20 ; MOVLW 0x01 ; ADDWF 0x20,W ; MOVWF 0x21
    let p = run(&[0x30FF, 0x00A0, 0x3001, 0x0720, 0x00A1], 0);
    assert_eq!(p.ram()[0x21], 0x00); // FF + 01 wraps to 0
    assert_eq!(p.ram()[0x03] & 0b001, 0b001); // carry set
    assert_eq!(p.ram()[0x03] & 0b100, 0b100); // zero set
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `nix develop --command cargo test --test byte_ops`
Expected: FAIL, `Pic14` not found.

- [ ] **Step 3: Implement state and the byte-op subset**

Append to `crates/sim/src/lib.rs`:

```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `nix develop --command cargo test --test byte_ops`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/sim/src/lib.rs crates/sim/tests/byte_ops.rs
git commit -m "feat(sim): core state and byte-oriented instructions"
```

---

### Task 4: Bit-oriented and literal/control instructions

**Files:**
- Modify: `crates/sim/src/lib.rs`
- Test: `crates/sim/tests/bit_lit.rs`

**Interfaces:**
- Consumes: `Pic14`, `read_f`/`write_f`, `set_z`/`set_c`/`set_dc`, `add_flags`, `pop_return` (Task 3).
- Produces: full 35-instruction interpreter; `exec_bit`, `exec_call_goto`, `exec_literal` fully implemented.

- [ ] **Step 1: Write the failing test**

`crates/sim/tests/bit_lit.rs`:

```rust
use pic14_sim::Pic14;

fn run(words: &[u16]) -> Pic14 {
    let mut p = Pic14::new(words.to_vec());
    p.run(1000);
    p
}

#[test]
fn btfs_skips_when_bit_set() {
    // MOVLW 0x04 ; MOVWF 0x20 ; BTFSC 0x20,2 ; MOVLW 0xAA ; MOVWF 0x21
    let p = run(&[0x3004, 0x00A0, 0x1920, 0x30AA, 0x00A1]);
    assert_eq!(p.ram()[0x21], 0x00); // bit 2 set -> skip the MOVLW 0xAA
}

#[test]
fn sublw_sets_carry_when_no_borrow() {
    // MOVLW 0x01 ; SUBLW 0x02  -> W = 2 - 1 = 1, C set
    let p = run(&[0x3001, 0x3C02]);
    assert_eq!(p.w(), 0x01);
    assert_eq!(p.ram()[0x03] & 0b001, 0b001);
}

#[test]
fn call_and_retlw_roundtrip() {
    // CALL 0x04 (0x2004) -> push return addr 1, jump to 4 ; at 4: RETLW 0x42 -> W=0x42, return to 1
    let mut p = Pic14::new(vec![0x2004, 0x0000, 0x0000, 0x0000, 0x3442]);
    p.run(1000);
    assert_eq!(p.w(), 0x42);
    assert_eq!(p.pc(), 0x01);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `nix develop --command cargo test --test bit_lit`
Expected: FAIL (stubs return `pc+1`).

- [ ] **Step 3: Implement bit, call/goto, and literal**

Replace the three stubs in `crates/sim/src/lib.rs`:

```rust
    fn exec_bit(&mut self, pc: u16, word: u16) -> u16 {
        let b = ((word >> 7) & 0x7) as u8;
        let f = (word & 0x7F) as usize;
        match (word >> 10) & 0x3 {
            0 => self.ram[f] &= !(1 << b), // BCF
            1 => self.ram[f] |= 1 << b,   // BSF
            2 => {
                if self.ram[f] & (1 << b) == 0 {
                    return pc + 2; // BTFSC skip if clear
                }
            }
            3 => {
                if self.ram[f] & (1 << b) != 0 {
                    return pc + 2; // BTFSS skip if set
                }
            }
            _ => unreachable!(),
        }
        pc + 1
    }
    fn exec_call_goto(&mut self, pc: u16, word: u16) -> u16 {
        let k = word & 0x7FF;
        self.ram[0x0A] = ((k >> 8) & 0x1F) as u8; // PCLATH
        if word & 0x0800 != 0 {
            k // GOTO
        } else {
            self.stack.push(pc + 1); // CALL
            k
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
                self.ram[0x0A] = ((ret >> 8) & 0x1F) as u8;
                return ret;
            }
            _ => unreachable!(),
        }
        pc + 1
    }
```

Also add the remaining byte-op arms to the `match op6` in `exec_byte` (Task 3 left them as
`other => panic!`), and the `rlf`/`rrf` helpers:

```rust
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
```

Add these two helpers next to `add_flags`:

```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `nix develop --command cargo test --test bit_lit`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/sim/src/lib.rs crates/sim/tests/bit_lit.rs
git commit -m "feat(sim): bit, literal, and control instructions"
```

---

### Task 5: INDF/FSR/PCL/PCLATH special-register semantics

**Files:**
- Modify: `crates/sim/src/lib.rs`
- Test: `crates/sim/tests/indirect.rs`

**Interfaces:**
- Consumes: `read_f`/`write_f` (Task 3) and `exec_call_goto` PCLATH (Task 4).
- Produces: `read_f`/`write_f` handle INDF (0x00 → RAM[FSR]) and PCL (0x02 → low byte of `pc`); `MOVWF PCL` performs a computed jump to `PCLATH:W`.

- [ ] **Step 1: Write the failing test**

`crates/sim/tests/indirect.rs`:

```rust
use pic14_sim::Pic14;

#[test]
fn indf_aliases_fsr() {
    // MOVLW 0x20 ; MOVWF 0x04 (FSR) ; MOVLW 0x55 ; MOVWF 0x00 (INDF) ; MOVF 0x20,W
    let mut p = Pic14::new(vec![0x3020, 0x0084, 0x3055, 0x0080, 0x0820]);
    p.run(1000);
    assert_eq!(p.w(), 0x55); // MOVWF INDF wrote RAM[0x20] via FSR=0x20
}

#[test]
fn movwf_pcl_computed_jump() {
    // MOVLW 0x02 ; ADDLW LOW(table) ; MOVWF PCL ; (fallthrough) ... ; table: RETLW 0x10/0x20
    // table is at word 4; LOW(table)=4; W = 2+4 = 6 -> jumps to word 6 = RETLW 0x20
    let mut p = Pic14::new(vec![0x3002, 0x3E04, 0x0082, 0x0000, 0x3410, 0x0000, 0x3420]);
    p.stack.push(0xFFFF); // fake return so RETLW doesn't underflow
    p.run(1000);
    assert_eq!(p.w(), 0x20);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `nix develop --command cargo test --test indirect`
Expected: FAIL (INDF/PCL not aliased).

- [ ] **Step 3: Implement the aliasing**

Replace `read_f`/`write_f` and the `MOVWF` byte-op arm in `crates/sim/src/lib.rs`:

```rust
    fn read_f(&self, f: usize) -> u8 {
        match f {
            0x00 => self.ram[self.ram[0x04] as usize], // INDF -> RAM[FSR]
            0x02 => (self.pc & 0xFF) as u8,            // PCL
            _ => self.ram[f],
        }
    }
    fn write_f(&mut self, f: usize, v: u8) {
        match f {
            0x00 => {
                let fsr = self.ram[0x04] as usize;
                self.ram[fsr] = v;
            }
            _ => self.ram[f] = v,
        }
    }
```

And in `exec_byte`'s `0x00` arm (MOVWF), add the PCL special case before `write_f`:

```rust
            0x00 => {
                if d == 1 {
                    if f == 0x02 {
                        let pclath = (self.ram[0x0A] as u16) & 0x1F;
                        return (pclath << 8) | (self.w as u16);
                    }
                    self.write_f(f, self.w);
                }
            }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `nix develop --command cargo test --test indirect`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/sim/src/lib.rs crates/sim/tests/indirect.rs
git commit -m "feat(sim): INDF/FSR and PCL/PCLATH indirect addressing"
```

---

### Task 6: gpasm cross-check integration test

**Files:**
- Test: `crates/sim/tests/gpasm_cross.rs`
- Create: `crates/sim/tests/fixtures/fib.asm`

**Interfaces:**
- Consumes: `parse_hex`, `Pic14` (Tasks 2–5).
- Produces: an integration test that assembles a hand-written program with `gpasm` and verifies our simulator's semantics agree, proving the oracle is real from the first commit.

- [ ] **Step 1: Write the fixture**

`crates/sim/tests/fixtures/fib.asm` — compute `0x20 = 0x20 + 0x21`, store to `0x22`:

```asm
    list p=16f877a
    radix hex
    org 0
    goto start
start:
    movf 0x20, W
    addwf 0x21, W
    movwf 0x22
    sleep
    end
```

- [ ] **Step 2: Write the failing test**

`crates/sim/tests/gpasm_cross.rs`:

```rust
use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

#[test]
fn agrees_with_gpasm_assembled_program() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", "fib.asm", "-o", "fib.hex"])
        .current_dir(dir)
        .output()
        .expect("run gpasm");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string(format!("{dir}/fib.hex")).expect("read hex");
    let mut p = Pic14::new(parse_hex(&hex));
    p.ram_mut()[0x20] = 0x12;
    p.ram_mut()[0x21] = 0x34;
    p.run(1000);
    assert_eq!(p.ram()[0x22], 0x46); // 0x12 + 0x34 = 0x46
    assert!(p.halted());
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `nix develop --command cargo test --test gpasm_cross`
Expected: FAIL if the fixture/flow is broken (e.g. missing fixture) — this is the first end-to-end check.

- [ ] **Step 4: Verify it passes**

Run: `nix develop --command cargo test --test gpasm_cross`
Expected: PASS — `0x12 + 0x34 = 0x46`, halted at `SLEEP`.

- [ ] **Step 5: Commit**

```bash
git add crates/sim/tests/gpasm_cross.rs crates/sim/tests/fixtures/fib.asm
git commit -m "test(sim): cross-check simulator against gpasm"
```

---

## Self-review notes

- **Spec coverage:** Phase 1 = simulator + gpasm cross-check (spec §3). The XC8 differential runner and YARPGen/cvise loop are *later* phases (they need a compiler to diff against / drive), and are deliberately not in this plan — they are phase 6 work. Snapshot (`insta`) wiring is deferred until the first text-emitting pipeline stage exists (phase 2), since there is nothing to snapshot in a simulator-only phase.
- **Type consistency:** `Pic14`, `parse_hex`, `read_f`, `write_f`, `write_d`, `add_flags`, `pop_return`, `exec_byte`, `exec_bit`, `exec_call_goto`, `exec_literal` names are stable across all tasks.
- **Placeholder scan:** Task 4 Step 3 lists the remaining byte-op arms by name (COMF/DECF/INCF/etc.) rather than full code — an executor fills these with the standard semantics. This is the one spot the plan trusts an executor's ISA knowledge; the spike's `spike/src/sim.rs` is the exact reference to copy from.
