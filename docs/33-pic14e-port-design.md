# 33: PIC14E (Enhanced Mid-range) port design

> **Approval status:** approach and phasing **pending user approval**.
> This document is the design of record for [issue #228](https://github.com/apojomovsky/epic-cc/issues/228).
> The implementation plan derives from it and does not exist yet.

**Target:** the PIC16F1937 / PIC16F1939 family (the 1933/1934/1936/1938
share the core with less flash and RAM; the PIC12F1xxx parts share the
core once it exists, noted but not scoped here). Chosen because
`pic16f193x-hal` already exists and is verified against XC8, so the
peripheral/register reference is real and in-repo, and because the
1937/1939 are the popular hobbyist Enhanced Mid-range parts.

**Definition of done:** parity with what the PIC14 backend supports
today. The existing e2e fixtures compile, assemble byte-for-byte
against `gpasm -p p16f1937`, and run correctly in the simulator, for
PIC14E.

---

## 1. Why this is a smaller job than the PIC18 port

The PIC18 port was a new core in almost every dimension: 16-bit words,
byte-oriented PC, two-word instructions, an access bank, hardware
multiply, `TBLRD`, two interrupt vectors. PIC14E keeps the PIC14
foundation and adds a layer on top. Every figure in this document is
working knowledge and must be confirmed against the PIC16F193X data
sheet (DS41364B) before it is hard-coded; items most worth checking
first are flagged `[VERIFY]`, matching the convention in
[`01-target-pic14.md`](01-target-pic14.md).

| PIC14 constraint | PIC14E |
|---|---|
| 14-bit instruction words | Same 14-bit words; the byte-oriented, bit-oriented and literal opcode families are **bit-identical encodings** (DS41364B Table 26-3 vs the classic set) |
| 8-level hardware stack | 16-level, still not addressable (DS41364B §2.4) |
| Bank via RP1:RP0 bits in STATUS | Dedicated 5-bit `BSR` register, `MOVLB k` (DS41364B §2.2, Table 26-3) |
| 16 bytes of `BANKSEL`-free common RAM (0x70-0x7F) | Same 16 common bytes, reachable from any bank (DS41364B §2.2) |
| 2K-word pages via `PCLATH<4:3>` | Same 2K-word pages, but `PCLATH` is 7 bits and `MOVLP k` loads it in one instruction (DS41364B §2.3) |
| `PCLATH` paging on every call and goto | Same, plus `BRA` (relative, ±256 words) and `BRW` (PC + 1 + W) for relocatable branches |
| No multiply instruction | Still none: no `MULWF`/`MULLW` in the instruction set (DS41364B Table 26-3) |
| `const` in flash via `RETLW` jump tables | Same `RETLW` mechanism, plus a new FSR-to-flash mapping (see D-5) |
| 8-bit FSR + IRP bit, objects must fit one bank window | 16-bit FSR0/FSR1 with `ADDFSR`, `MOVIW`/`MOVWI` pre/post inc/dec and indexed `[k]INDFn`; a **linear data region** (0x2000-0x29AF) concatenates the banks' GPR blocks, so buffers can span banks (DS41364B §2.5) |
| One interrupt vector, manual context save | One vector at 0x0004, **hardware context save** of W/STATUS/BSR/FSR0/FSR1/PCLATH to shadow registers, restored by `RETFIE` (DS41364B §4.1) |
| 4 banks | Up to 32 banks of 128 bytes (DS41364B §2.2) |

### What carries over untouched

`irparse` and `ir` (LLVM IR text is not target-specific), `wholeprog`,
`callgraph`, `legalize`, and the `fuzz` differential harness. The
verification strategy survives whole: `gpasm`, `gpsim` and XC8 all
support the 1937, so all three oracles carry over. The PIC14 integer
spine's ALU/literal/bit emitters, the `PCLATH` paging machinery, the
`__mul_u8`/`__udiv_u8` shift-add routines, the soft-float
routines, and the `RETLW` const-table machinery all carry over with
mnemonic-for-mnemonic substitution.

### What gets deleted or simplified

The `fsr_window` straddling checks (M9): a PIC14E FSR is 16 bits and
the linear data region spans banks, so the "object must fit one bank
window" constraint stops existing. The manual ISR save/restore
prologue: the hardware shadow registers do it. The `BANKSEL` skip-window
hazard: `MOVLB` is a single instruction that does not touch flags, so
the skip-sensitive `BANKSEL` expansion classic PIC14 needs does not
exist here.

---

## 2. Decisions

### D-1: A third parallel backend crate, `isel-pic14e`

**Decision:** `isel` stays as the classic PIC14 backend, `isel-pic18`
stays as the PIC18 backend, and a new `isel-pic14e` crate holds the
Enhanced Mid-range backend. The driver selects by device. `iselcore`
gains nothing new in v1.

**Rationale.** The open question this ticket poses, "does `isel-pic14e`
share more with `isel` or with `isel-pic18` architecturally", has a
clear answer from the ISA survey: **it shares the emitter surface with
`isel` (classic PIC14) and borrows the FSR shape from `isel-pic18`.**

- The byte-oriented, bit-oriented and literal opcode families are
  bit-identical encodings to classic PIC14 (DS41364B Table 26-3). The
  ALU emitter surface (`ADDWF`/`SUBWF`/`IORWF`/... with the `d`
  destination bit, `MOVLW`/`ADDLW`/... literals, `BCF`/`BSF`/`BTFSC`/
  `BTFSS` bit ops) is the same code with the same encodings. This is
  the bulk of the integer spine, and it is `isel`'s, not `isel-pic18`'s.
- The paging machinery is `isel`'s: 2K-word pages, `PCLATH` tracking,
  the M11 restore pair. `MOVLP` makes the load a single instruction
  instead of `MOVLW`+`MOVWF`, but the dataflow pass is the same shape.
- The FSR/indirect machinery is `isel-pic18`'s shape: 16-bit FSRs,
  indexed addressing. But the instructions differ (`MOVIW [k]INDFn`
  is one instruction where PIC18 needs `MOVLW`+`MOVF PLUSW2`), and the
  linear data region changes what the allocator must guarantee, so the
  emitters are PIC14E-specific, not shared.
- The banking pass is a third variant: track `BSR`, insert `MOVLB`,
  common RAM needs no bank. Structurally closest to PIC18's `MOVLB`
  tracking, but with no access-bank `a` bit and with the common-RAM
  window, so it is neither `banking`'s RP1:RP0 pass nor `isel-pic18`'s
  BSR+access-bank pass.

**Rejected, a `Target` trait over one generic `isel`.** Same reasoning
as the PIC18 port's D-1: the abstraction would degrade into
`emit_move(dst, src)` wrappers that obscure the point of the port, and
it would refactor the working, tested backend first.

**Rejected, one crate parameterised by `Device`, branching
internally.** Same scatter objection as PIC18's D-1.

**Consequence, stated plainly:** the soft-float routines and the
mul/div routines are written a third time. This is accepted and cheap:
they are 1:1 ports of the verified PIC14 bodies with a substitution
table, exactly as the PIC18 port did (ADR-015).

### D-2: Static allocation in v1, `Slot`-ready, linear region for big buffers

**Decision:** v1 ports the existing call-graph overlay allocator.
Recursion remains a compile error. Local addressing routes through the
existing `Slot` abstraction (already in `iselcore` from the PIC18
port). The allocator gains the device's linear data region as a
placement option for objects that do not fit one bank.

**Rationale.** PIC14E has no addressable stack (16-level hardware
stack, not addressable), so static overlay allocation is the only
option, same as classic PIC14. The new capability is the linear data
region (DS41364B §2.5.2): FSR addresses 0x2000-0x29AF map to the
concatenated 80-byte GPR blocks of all banks, so a buffer larger than
80 bytes can be placed there and accessed through FSR with no bank
switching. This is the PIC14E answer to PIC18's frame pointer: not a
stack, but a bank-spanning data window. The allocator should place
oversized objects there and let the FSR emitters address them linearly.

**Why the abstraction already exists.** `Slot` landed with the PIC18
port (P0, `iselcore`). PIC14E consumes it unchanged; no retrofitting.

### D-3: Device support is data, via the existing registry

**Decision:** the 193x family lands as TOMLs under
`crates/device/devices/` (`p16f1937.toml`, `p16f1939.toml`, and the
rest of the family), generated by `scripts/gen-device.py`, which
already maps the EDC/ini architecture `16exxx`/`PIC14E` to
`core = "pic14e"`. The firewall stays until the backend lands.

**Rationale.** ADR-019 already settled device-as-data and the
file-per-device TOML registry; `gen-device.py` already knows the
`pic14e` core string. The 193x TOMLs are reviewable before the backend
exists, which is exactly the ADR-019 posture: the data should be
reviewable before the backend, and the driver's `core pic14e which has
no backend yet` refusal is the firewall (ADR-019 consequences, #91).

The 1937 TOML needs `[VERIFY]` against DS41364B: flash 8192 words,
SRAM 512 bytes, 16-level stack, single vector at 0x0004, config words
at 8007h/8008h (word addresses, 14-bit words), and the bank map
(32 banks x 128 bytes, 16 common bytes at 0x70-0x7F). The 1939 adds
16384 words flash and 1024 bytes SRAM.

### D-4: Interrupts: single vector, hardware context save

**Decision:** target the single vector at 0x0004 with the hardware
shadow-register context save. No priority model exists on this core
(no `IPEN`), so there is no compatibility-mode question like PIC18's
P5 had.

**Rationale.** DS41364B §4.1: on interrupt entry the hardware saves
W/STATUS/BSR/FSR0/FSR1/PCLATH to shadow registers and restores them on
`RETFIE`. The ISR needs no manual save/restore prologue, which is
simpler than both existing backends (PIC14 saves manually, PIC18 has
the two-vector priority question). The `epic_dispatch_all_irqs`
fan-out shape carries over from `pic16f193x-hal`'s proven
implementation: read INTCON/PIR1/PIR2/PIR3 once, dispatch only the
sources whose bits are set, each handler clears its own flag.

One consequence to record: because the shadow registers restore on
`RETFIE`, any W/STATUS/BSR/FSR/PCLATH value the ISR leaves behind is
lost. The compiler's ISR codegen must not rely on values surviving
across the ISR boundary, and the `_isr` frame copies (the PIC18 P5
mechanism) apply unchanged.

### D-5: `const` in flash via `RETLW` in v1; the FSR-flash mapping is a follow-up

**Decision:** v1 ports the classic PIC14 `RETLW` const-table machinery
unchanged. The PIC14E-native FSR-to-flash mapping (DS41364B §2.5.3:
setting bit 7 of FSRnH maps the FSR to program flash, read through
`MOVIW`) is a documented follow-up, not v1.

**Rationale.** The `RETLW` machinery is proven, carries over with no
changes, and de-risks the port. The FSR-flash mapping is the PIC14E
analog of PIC18's `TBLRD`: it would delete the 256-byte window and
511-byte ceiling the same way. But it is a new mechanism with its own
simulator and assembler surface, and the port already has enough new
surface. Land `RETLW` first, then replace it when the FSR machinery is
settled.

### D-6: The `MOVLP`/`PCLATH` paging pass carries over from `isel`

**Decision:** the PIC14E paging pass is `isel`'s PCLATH dataflow with
`MOVLP k` as the load instruction. `BRA`/`BRW` are used where the
relative form fits, which reduces the number of pages that need
management.

**Rationale.** The 1937's 8192 words is four 2K-word pages, the same
page geometry as the 877A; the 1939's 16384 words is eight. The
`PCLATH<4:3>` → `PC<12:11>` mapping for `CALL`/`GOTO` is identical to
classic PIC14 (DS41364B §2.3.2). `MOVLP` loads all 7 PCLATH bits in
one instruction, which is strictly simpler than the classic
`MOVLW`+`MOVWF PCLATH` pair. `BRA` (PC + 1 + signed 9-bit) and `BRW`
(PC + 1 + W) are new and give the backend relocatable branches, which
the peephole pass can use to elide page management on intra-page
branches.

---

## 3. Architecture

### Crate layout after the port

```
crates/
  ir/            unchanged   the IR data model
  irparse/       unchanged   LLVM IR text parser
  wholeprog/     unchanged   module merging
  callgraph/     unchanged   call graph, cycle detection, depth check
  legalize/      unchanged   routine selection is already device-aware
  alloc/         adapted     linear data region as a placement option
  device/        adapted     p16f1937.toml etc. (data only, gen-device)
  iselcore/      unchanged   Slot, ssa_key, parse_map, Base, resolve_pointers
  isel/          unchanged   the classic PIC14 backend
  isel-pic18/    unchanged   the PIC18 backend
  isel-pic14e/   NEW         the Enhanced Mid-range backend
  banking/       adapted     PIC14 RP1:RP0 path kept; BSR path added
  peephole/      adapted     PIC14 PCLATH path kept; PIC14E pass is separate
  asm/           adapted     PIC14 encoder kept; PIC14E encoder added
  sim/           adapted     PIC14 core kept; PIC14E core added
  driver/        adapted     selects the pipeline by device
  fuzz/          adapted     device flag threaded through; harness unchanged
```

### The device profile

The 1937 TOML follows the existing schema (ADR-019). Sketch, every
constant `[VERIFY]` against DS41364B:

```toml
name = "p16f1937"
core = "pic14e"
flash_words = 8192          # DS41364B Table 1-1
ram_banks = [ ... ]         # 32 banks x 128 bytes, GPR blocks per bank
common_ram = [0x0070, 0x007F]
stack_depth = 16            # DS41364B §2.4
interrupt_vectors = [0x0004]
linear_ram = [0x2000, 0x29AF]   # NEW field: the linear data region

[config]
base_byte_addr = 0x8007     # word address, 14-bit words (DS41364B §10.1)
num_bytes = 4               # CONFIG1 + CONFIG2
```

The `linear_ram` field is the one schema addition: the allocator needs
to know the linear region exists and its extent. It is a hardware
fact, cross-checkable against the DFP like the rest.

### Data flow

The ten-stage pipeline is unchanged in shape. The branch is at exactly
one place, in the driver, and it selects a backend rather than
threading a boolean through the backend.

```
.c → clang → .ll → irparse → wholeprog → legalize → callgraph → alloc
                                                                 ↓
                    device selects: isel | isel-pic18 | isel-pic14e
                                                                 ↓
                                              banking → peephole → asm → .hex
```

---

## 4. Phases

Nine phases, front-loaded on de-risking, mirroring the PIC18 port's
phase list as the effort reference. Each has an acceptance criterion
that is a test, not a judgement.

| Phase | Deliverable | Acceptance |
|---|---|---|
| **P0** | 193x device TOMLs via `gen-device.py`; `linear_ram` field; firewall stays. No backend code. | `gen-device --check` clean; the TOMLs validate through `build.rs`; the driver still refuses `pic14e` with the existing message. |
| **P1** | PIC14E `asm` encoder and `sim` core. Hand-written `.asm` inputs only, no codegen. | `gpasm -p p16f1937` byte-for-byte HEX match, plus simulator tests per instruction group (the new ASRF/LSLF/LSRF, MOVLB/MOVLP, BRA/BRW/CALLW, ADDFSR/MOVIW/MOVWI). |
| **P2** | Integer spine: `isel-pic14e`, BSR banking, `Slot::Direct`. | `add.c`, `scalar.c`, `overlay.c`, `banked.c` |
| **P3** | Pointers, arrays, structs via FSR0/1; linear region for oversized buffers. | `ptr_probe.c`, `array.c`, `structs.c`, `banked_ptr.c`; a buffer spanning two banks via the linear region |
| **P4** | `const` in flash via `RETLW` (carried over). | `const_table.c`, `ptr_probe.c`; the 511-byte ceiling stays (D-5) |
| **P5** | Interrupts: single vector, hardware context save, `_isr` frame copies. | `interrupt.c`, `interrupt_gate.c`; the ISR needs no manual save/restore |
| **P6** | 32-bit `long`, mul/div routines (carried over from PIC14). | `long.c`, `muldiv.c`, `interrupt_mul.c` |
| **P7** | Soft-float: f32 routines (1:1 port of the PIC14 bodies). | `float.c` (out1=0x3F99999A, out2=0x41100000, out3=0x3EAAAAAB) |
| **P8** | Fuzz gate: device-threaded differential runner on PIC14E. | `pic14e.rs` fast (8) and full corpora (200, 50, 50) clean on the PIC14E sim via `--device` |

**P0 deserves emphasis.** It is pure data plus one schema field, with
the existing suite as its oracle. The TOMLs are reviewable before any
backend code exists, which is the ADR-019 posture, and the firewall
stays until P2 lands.

**P1 before P2 is deliberate.** Same reasoning as the PIC18 port: a P2
failure must never be ambiguous between "chose the wrong instruction"
and "emitted the wrong bits". The new instructions (the shifts, the
FSR moves, the branches) are exactly where that ambiguity would bite.

**P5 is simpler than either existing backend's.** No manual save
(PIC14), no priority question (PIC18). The hardware context save is the
whole story.

---

## 5. Prerequisites and backlog interaction

### Must land before P2

**None hard.** The PIC18 port's blockers (#9, #10, #14) all landed
during that port. `Global.addr` is 16-bit, constant folding exists, and
the differential fuzzer is device-threaded. The `linear_ram` device
field is new but is data, not plumbing.

### Obsoleted by the port, do not invest in them first

| Issue | Why the port obsoletes it |
|---|---|
| [#7](https://github.com/apojomovsky/epic-cc/issues/7) FSR globals inside bank windows | PIC14E FSRs are 16-bit and the linear region spans banks |
| [#6](https://github.com/apojomovsky/epic-cc/issues/6) Bank-0 restriction on routine slots | The 16 common bytes (0x70-0x7F) are bank-independent, same as PIC14's, and the linear region widens placement |

They remain worth doing **for the PIC14 backend on its own merits**.
The claim here is only about ordering relative to the port.

### Either order

[#11](https://github.com/apojomovsky/epic-cc/issues/11) (IEEE754 edge
cases): the corpus is reusable for the PIC14E float routines, same
argument as the PIC18 port made.

---

## 6. Testing strategy

The port inherits the four verification layers rather than inventing
any.

1. **Our own simulator**, extended with a PIC14E core. Deterministic,
   embeddable in `cargo test`.
2. **`gpasm` byte-for-byte cross-check**, against `-p p16f1937`. This
   is P1's entire acceptance criterion and the reason P1 precedes P2.
3. **The e2e acceptance programs**, recompiled for PIC14E. They
   are the parity definition, phase by phase, in the P2-P7 table above.
4. **Differential fuzzing** against host clang, with the seed corpora.

Two additions specific to a port, both inherited from the PIC18 port:

- **The PIC14 suite is a regression oracle for P0.** Pure data plus
  one schema field that keeps every existing test passing is the
  strongest possible evidence that the device abstraction did not
  change behaviour.
- **Cross-target differential.** The same C source compiled for both
  targets must produce the same observable results in the simulator.

The `pic16f193x-hal` register/IRQ work is a running cross-check, not a
test oracle: its `docs/ARCHITECTURE.md` records what XC8 actually does
on this core (BSR auto-banking of literal SFR tokens, the FSR1:INDF1
RMW trap that silently addressed the wrong byte, the `movlb 1` +
`iorwf PIE1,f` fix shape). The compiler must produce code that avoids
the trap the HAL hit, and the HAL's verified register facts are the
reference for the device TOML's SFR table.

---

## 7. Risks and open questions

**Every `[VERIFY]` item above.** Memory map, bank layout, linear region
extent, stack depth, vector address, config word addresses, flash
size. Confirm against DS41364B before hard-coding into `device`.

**The FSR address space has three regions.** Traditional data memory
(0x000-0xFFF), the linear data region (0x2000-0x29AF), and program
flash (bit 7 of FSRnH set, 0x8000+), with reserved gaps between
(DS41364B §2.5). The simulator must model all three and the
one-extra-cycle cost of flash access. Getting this boundary wrong is a
whole class of P1 bugs, which is precisely why P1 is gated on a
byte-for-byte `gpasm` match.

**The `MOVIW`/`MOVWI` pre/post inc/dec forms.** `++INDFn`, `--INDFn`,
`INDFn++`, `INDFn-`, and the indexed `[k]INDFn` form each have a
distinct encoding and a distinct FSR side effect (DS41364B Table 26-3).
The assembler's encoding and the simulator's FSR update must agree;
this is the PIC14E analog of the PIC18 two-word-instruction risk.

**The shadow-register interrupt model.** The hardware saves
W/STATUS/BSR/FSR0/FSR1/PCLATH on entry and restores on `RETFIE`
(DS41364B §4.1). The compiler must not rely on ISR-side register
values surviving, and the `_isr` frame copies must be disjoint from
the main frames, same as PIC18's P5. The one thing to verify: re-read
§4.1 to confirm the shadow save covers every interrupt entry on this
core, which has no priority levels.

**The clang side is assumed unchanged.** `-target msp430` remains the
datalayout proxy, same as PIC18 (8-bit `char`, 16-bit `int`,
`-fpack-struct`). `[VERIFY]` that the PIC14E pointer width (16-bit
FSR-based, like PIC18's) does not argue for a different proxy.

**The linear region and the allocator.** Placing an oversized object
in the linear region changes its address from a banked 7-bit offset to
a 16-bit FSR address. Every consumer of the address (the FSR emitters,
the `Slot` machinery) must handle both. This is the one place the port
touches `alloc` and `iselcore`'s contract, and it is P3's first design
question.
