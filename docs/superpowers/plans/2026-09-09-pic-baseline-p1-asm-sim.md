# PIC baseline P1: asm encoder + sim core

Ticket: epic-cc#324. Design of record: docs/37-pic-baseline-port-design.md
(§1 "What is adapted", §2 D-2, §3 phase table). Depends on P0 (epic-cc#323,
merged). Same shape of work docs/29 P1 and docs/33 P1 did for PIC18 and
PIC14E.

## Goal

- `asm`: a 12-bit-word encoder for the 33-instruction baseline ISA
  (PIC14's 35 minus `SUBLW` and `RETURN`, DS41236E Table 8-2), plus the
  `assemble_words` arm for `Core::PicBaseline`.
- `sim`: a `PicBaseline` core: the baseline register file with
  `FSR`/`INDF` indirect addressing where `FSR<5>` is both the indirect
  offset's bank select and the direct-addressing bank select (D-2), the
  2-level hardware call/return stack (D-4), no interrupts.
- The D-2 ordering-hazard acceptance test: hand-written `.asm` doing a
  direct access to bank 1, then an indirect access through a live pointer
  expecting bank 0 (and the reverse ordering), asserting the
  `BCF`/`BSF FSR,5` reassertion sequence produces the right effective
  addresses in `sim`.
- gpasm byte-for-byte cross-check against `p12f509.inc`
  (`gpasm-1.5.2`, present in the pinned dev image).

No `isel-pic-baseline` and no `driver` integration in this phase: the
driver firewall stays (P2's job).

## ISA facts (all confirmed against gpasm 1.5.2 probes, 2026-09-09)

Table 8-2 encodings, 12-bit word, MSb first:

| Group | Mnemonic | Opcode |
|---|---|---|
| byte | ADDWF f,d | 0001 11df ffff |
| byte | ANDWF f,d | 0001 01df ffff |
| byte | CLRF f | 0000 011f ffff |
| byte | CLRW | 0000 0100 0000 |
| byte | COMF f,d | 0010 01df ffff |
| byte | DECF f,d | 0000 11df ffff |
| byte | DECFSZ f,d | 0010 11df ffff |
| byte | INCF f,d | 0010 10df ffff |
| byte | INCFSZ f,d | 0011 11df ffff |
| byte | IORWF f,d | 0001 00df ffff |
| byte | MOVF f,d | 0010 00df ffff |
| byte | MOVWF f | 0000 001f ffff |
| byte | NOP | 0000 0000 0000 |
| byte | RLF f,d | 0011 01df ffff |
| byte | RRF f,d | 0011 00df ffff |
| byte | SUBWF f,d | 0000 10df ffff |
| byte | SWAPF f,d | 0011 10df ffff |
| byte | XORWF f,d | 0001 10df ffff |
| bit | BCF f,b | 0100 bbbf ffff |
| bit | BSF f,b | 0101 bbbf ffff |
| bit | BTFSC f,b | 0110 bbbf ffff |
| bit | BTFSS f,b | 0111 bbbf ffff |
| lit | ANDLW k | 1110 kkkk kkkk |
| lit | CALL k | 1001 kkkk kkkk |
| lit | CLRWDT | 0000 0000 0100 |
| lit | GOTO k | 101k kkkk kkkk |
| lit | IORLW k | 1101 kkkk kkkk |
| lit | MOVLW k | 1100 kkkk kkkk |
| lit | OPTION | 0000 0000 0010 |
| lit | RETLW k | 1000 kkkk kkkk |
| lit | SLEEP | 0000 0000 0011 |
| lit | TRIS f | 0000 0000 0fff |
| lit | XORLW k | 1111 kkkk kkkk |

- `f` is 5 bits (0-31), `b` is 3 bits, `k` is 8 bits (9 for GOTO).
- **Destination default is d = 1 (file)** on baseline, unlike classic
  PIC14's W default. gpasm 1.5.2 confirms: `MOVF 0x10` assembles to
  0x0230 (d=1), with Message[305] "Using default destination of 1".
  The PIC14E encoder already implements this default; baseline copies it.
- `GOTO` is `101k kkkk kkkk`: bit 11 is part of the 9-bit literal
  (0x0BFF for `GOTO 0x1FF`), unlike PIC14's `0x2800 | k`.
- `CALL`/`RETLW` are 2-cycle; the sim models the extra cycle as a
  step-counting detail only if the existing cores do (they do not count
  cycles per instruction, only steps; keep parity with Pic14).
- `TRIS f` and `OPTION` are control ops with no register operand in the
  file map (TRISGPIO/OPTION are not addressable SFRs, DS41236E Table 4-1).
  The sim models them as write-only shadow registers (TRISGPIO at
  `0x80`, OPTION at `0x81`), mirroring how Pic14e models OPTION/TRISx.

## Memory model (DS41236E Figure 4-4, section 4.9)

- SFR block 0x00-0x06 (INDF, TMR0, PCL, STATUS, FSR, OSCCAL, GPIO)
  mirrors into every bank (gputils `12f509_g.lkr` SHAREBANK sfrs
  0x0-0x6 and 0x20-0x26).
- Bank 0 GPR 0x10-0x1F, bank 1 GPR 0x30-0x3F (DATABANK gpr0/gpr1).
- Shared GPR 0x07-0x0F visible from both banks (SHAREBANK gprnobnk).
- Direct addressing: `f` selects the location within the bank chosen by
  `FSR<5>` (Figure 4-7). Physical address = `(FSR<5> << 5) | f`.
- Indirect addressing: `INDF` reads/writes `RAM[FSR]` where FSR is the
  full 6-bit flat address (bank bits + offset, Figure 4-7). FSR<7:6>
  unimplemented, read as 1 on the 509.
- The 2-level stack: CALL pushes PC+1 (shifting level 1 to level 2);
  RETLW pops level 1 into PC and copies level 2 into level 1. Overflow
  drops the oldest; underflow returns 0 (no status bits, DS41236E
  section 4.8 note 1). Model as a fixed `[u16; 2]` with a level counter,
  matching the datasheet's shift semantics exactly.
- PC: 11 bits on the 509. GOTO takes bits 8:0 from the word and bit 9
  from STATUS PA0 (bit 5). CALL and PCL-modifying instructions force
  PC<8> to 0 (section 4.7). PCL is PC<7:0> at 0x02; a PCL write
  (MOVWF PCL) sets PC<7:0> = W, PC<8> = 0, PC<9> = PA0.
- STATUS at 0x03: GPWUF(7), -(6), PA0(5), TO(4), PD(3), Z(2), DC(1),
  C(0). TO/PD are read-only; a write to STATUS only reaches the
  writable bits (section 4.4: "the result of an instruction with the
  STATUS register as destination may be different than intended").
  Keep the Pic14e model: writes preserve TO/PD, and Z/DC/C are set by
  ALU logic. PA0 is writable (it is the page preselect bit).

## asm changes (crates/asm/src/lib.rs)

- `pub fn assemble_pic_baseline(src: &str) -> Vec<u16>`: two-pass like
  `assemble`, reusing `assemble_first_pass` (labels/org/equ/directives
  are core-independent). Pass 2 calls `encode_pic_baseline`.
- `fn encode_pic_baseline(line, sym) -> u16`: the Table 8-2 match.
  - `f` operand: 5-bit range assert (0x00-0x1F), symbol-resolved like
    the PIC14 `f` closure.
  - `d` default = 1 (file), `W` = 0, `F` = 1.
  - Bit ops: join remaining tokens (`STATUS, 5` split across
    whitespace), split on comma, `b` 3-bit assert.
  - `GOTO`: 9-bit literal, `0x0800 | (k & 0x1FF)`.
  - `CALL`: 8-bit literal, `0x0900 | (k & 0xFF)`.
  - `TRIS f`: `0x0000 | (f & 0x7)`.
  - Literals: `parse_lit` (LOW/HIGH/PAGE/UPPER already resolve through
    the symbol table; PAGE is meaningless on a 2-page core but harmless
    to keep for symmetry with the shared pass-1 helpers).
- `assemble_words`: replace the `PicBaseline` panic arm with
  `assemble_pic_baseline(src)`. The firewall test
  (`crates/asm/tests/pic_baseline_firewall.rs`) must be updated: it
  currently asserts the panic; it becomes a real assembly test.

## sim changes (crates/sim/src/lib.rs)

- `pub struct PicBaseline` with `device: &'static Device` (for
  `fsr_bank_bits` and `ram_banks`), `prog: Vec<u16>`, `ram: [u8; 64]`
  (the 509's full 6-bit data space), `w`, `pc: u16`, `stack: [u16; 2]`,
  `stack_level: u8`, `halted: bool`, plus write-only shadow registers
  for TRISGPIO/OPTION (or a small `[u8; 2]`).
- `PicBaseline::with_device(device, prog)` asserting
  `device.core == Core::PicBaseline`.
- Addressing:
  - `direct_addr(f)`: `(fsr_bank() << 5) | f` where `fsr_bank()` is
    `(ram[0x04] >> 5) & 0x01` (masked by `fsr_bank_bits`).
  - `indirect_addr()`: `ram[0x04] & 0x3F` (the full flat address).
  - `read_f`/`write_f`: INDF (0x00) routes through `indirect_addr`;
    PCL (0x02) reads `pc & 0xFF`; STATUS (0x03) writes preserve TO/PD;
    everything else through `direct_addr`.
- `step()`: decode by top 2 bits like Pic14 (`0` byte, `1` bit, `2`
  call/goto, `3` literal) but with the baseline opcode map. The
  byte-op table is a different layout from PIC14 (e.g. ADDWF is
  `0001 11df ffff` = op6 0x1C, not 0x07), so the exec functions are
  written fresh, not adapted.
- `exec_literal`: CALL pushes `pc + 1` onto the 2-level stack; RETLW
  pops and copies level 2 to level 1; GOTO computes
  `((STATUS & 0x20) << 4) | (k & 0x1FF)`; MOVLW/ANDLW/IORLW/XORLW set
  Z per the table; SLEEP halts; CLRWDT/OPTION/TRIS are control ops.
- `exec_byte`: the 18 byte ops with their exact status effects
  (ADDWF/SUBWF set C/DC/Z; ANDWF/IORWF/XORWF/COMF/DECF/INCF/MOVF/CLRF/
  CLRW set Z; RLF/RRF set C; DECFSZ/INCFSZ skip; SWAPF sets nothing).
- `exec_bit`: BCF/BSF/BTFSC/BTFSS, skip = pc + 2.
- Public accessors mirroring Pic14: `ram()`, `ram_mut()`, `w()`,
  `pc()`, `halted()`, `run(max_steps)`, `step()`, `fsr()`, `status()`.
- `parse_hex` is shared (same wire format, 12-bit words little-endian
  at word*2); no new parser needed.

## Tests

- `crates/asm/tests/gpasm_pic_baseline.rs`: fixture
  `fixtures/pic_baseline_probe.asm` covering every instruction group
  (all 33 mnemonics, d=0/d=1/omitted, bit ops, GOTO/CALL literals,
  TRIS/OPTION, register equates via `#include <p12f509.inc>`),
  assembled by us and by `gpasm -p p12f509`, HEX compared
  byte-for-byte. Same shape as `gpasm_pic14e.rs`.
- `crates/sim/tests/pic_baseline.rs`: one test per instruction group,
  mirroring `pic14e.rs`'s structure: byte ops (flags, d routing),
  bit ops, literals, CALL/RETLW stack behavior (2-level shift,
  underflow), GOTO page bit, PCL write, direct bank selection via
  `BSF/BCF FSR,5`, indirect addressing through INDF, shared GPR
  mirroring, TRIS/OPTION shadow writes, SLEEP halt.
- `crates/sim/tests/pic_baseline_d2.rs`: the acceptance test. Two
  orderings:
  1. Direct access to bank 1 (`BSF FSR,5; MOVWF 0x10` -> physical
     0x30), then indirect access through a live pointer into bank 0
     (`MOVLW 0x10; MOVWF FSR; MOVF INDF,W` -> reads 0x10, not 0x30).
  2. The reverse: pointer into bank 1 first, then direct access to
     bank 0 after `BCF FSR,5`.
  Each asserts the effective addresses land in the right physical
  cells, proving the `BCF`/`BSF FSR,5` reassertion sequence (D-2)
  works in the simulator.

## Verification

- `make test CRATE=asm` and `make test CRATE=sim` in the container.
- `make check-warnings` clean.
- The D-2 test fails if the sim resolves either addressing mode with
  the wrong bank bits (it is the "not yet simulator-proven" gap D-2
  flags).

## Out of scope

- `isel-pic-baseline` (P2), driver backend arm (P2), `p12f508`/
  `p16f505` TOMLs, interrupts (none on this core), `SUBLW`/`RETURN`
  idioms (D-6, peephole/isel-level).
