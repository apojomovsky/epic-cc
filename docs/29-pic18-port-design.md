# 29: PIC18 port design

> **Approval status:** approach and phasing **approved by the user on 2026-08-18**.
> This document is the design of record for [issue #26](https://github.com/apojomovsky/epic-cc/issues/26).
> The implementation plan derives from it and does not exist yet.

**Target:** the PIC18F4550 / PIC18F2550 family (the 4455 / 2455 share the core with less
flash). Chosen because they are the popular hobbyist PIC18 parts, and because they run the
standard PIC18 core, so the work generalises to the rest of the line instead of being
specific to one part.

**Definition of done:** parity with what the PIC14 backend supports today. The fifteen
existing e2e fixtures compile, assemble byte-for-byte against `gpasm`, and run correctly in
the simulator, for PIC18.

---

## 1. Why this is a smaller job than a second compiler

Almost every hard problem in this compiler exists because of a PIC14 limitation, and PIC18
relaxes each one.

| PIC14 constraint | PIC18 |
|---|---|
| 8-level hardware stack, not addressable | 31 levels, readable via TOSU/TOSH/TOSL, plus PUSH/POP |
| No addressable stack: no frames, no recursion | FSR0/1/2 with `PLUSW` indexed addressing supports a real frame pointer |
| 16 bytes of `BANKSEL`-free common RAM | Access Bank, selected by a bit in the instruction itself |
| `const` in flash only via `RETLW` jump tables | `TBLRD` reads program memory directly |
| `PCLATH` paging on every call and goto | 20-bit `GOTO`/`CALL`, no page boundaries |
| No multiply instruction | Single-cycle hardware 8x8 multiply into PRODH:PRODL |
| Every copy goes through W, two instructions | `MOVFF` moves memory to memory in one |
| Bank via RP1:RP0 bits in STATUS | Dedicated `BSR` register, `MOVLB` |
| One interrupt vector | Two vectors with priority levels |

**Every figure in this document is working knowledge and must be confirmed against the
PIC18F2455/2550/4455/4550 data sheet (DS39632) and the PIC18 family reference manual before
it is hard-coded.** Items most worth checking first are flagged `[VERIFY]`, matching the
convention in [`01-target-pic14.md`](01-target-pic14.md).

### What carries over untouched

`irparse` and `ir` (LLVM IR text is not target-specific), `wholeprog`, `callgraph`, and the
`fuzz` differential harness. The verification strategy survives whole: `gpasm`, `gpsim` and
XC8 all support PIC18, so all three oracles carry over.

### What gets deleted

The `RETLW` const-table machinery including the 256-byte window and chunk chaining; most of
`peephole`, which today is almost entirely `PCLATH` elision; `__mul_u8` outright; and the
`fsr_window` straddling checks, since PIC18 FSRs are 12 bits and span the data space.

---

## 2. Decisions

### D-1: Parallel backend crates, not a generic backend

**Decision:** `isel` stays as the PIC14 backend, untouched. A new `isel-pic18` crate holds
the PIC18 backend. A small shared crate holds only the scaffolding proven to be
target-independent. The driver selects by device.

**Rationale.** An inventory of `isel`'s 54 functions splits three ways: six are PIC14-only
machinery that PIC18 deletes, four or five are genuinely target-independent scaffolding
(dominator computation, phi edge bookkeeping, block labelling, slot key naming, map
parsing), and roughly forty are emitters that do the same job with different instructions.

That last group is what decides the question. Sharing it requires abstracting over the
instruction set itself, and the differences are exactly what a shared trait would hide:
`MOVFF` collapses a two-instruction PIC14 copy into one, and hardware `MUL` deletes a loop
body rather than shortening it. The abstraction would degrade into `emit_move(dst, src)`
wrappers that obscure the whole point of the port.

**Rejected, a `Target` trait over one generic `isel`.** Requires refactoring 5,000 lines
of working, tested code before a single line of PIC18 exists, and produces a leaky
abstraction over two instruction sets that genuinely differ.

**Rejected, one crate parameterised by `Device`, branching internally.** This is
`if device.is_pic18 { … } else { … }` scattered through 5,000 lines. It puts the working
PIC14 path at risk on every PIC18 edit, and it is the same scatter that D-3 rejects for
device data.

**Consequence, stated plainly:** the nine soft-float routines will be written twice. This
is accepted. Those routines want rewriting for PIC18 anyway, to use hardware `MUL` in the
mantissa multiply, so a shared implementation would have been rewritten in practice.

### D-2: Static allocation in v1, but `Slot`-ready

**Decision:** v1 ports the existing call-graph overlay allocator. Recursion remains a
compile error. But local addressing is routed through a `Slot` abstraction from day one,
never a bare address, so frame-relative allocation can be added later without touching the
emitters.

**Rationale.** The correct end state is a **hybrid**, not a software stack: static
allocation for ordinary functions, a real frame only for functions that are recursive or
explicitly reentrant. This is what XC8 does for PIC18, and it follows from the instruction
set. On the standard PIC18 instruction set there is no literal-offset indexed addressing, so
a frame-relative local read costs two instructions:

```asm
    MOVLW   offset
    MOVF    PLUSW2, W       ; effective address = FSR2 + W
```

against one for a statically allocated local (`MOVF 0x2F, W`). A pure software stack would
double the instruction count of every local access in every function to buy recursion that
embedded C rarely uses.

**Why the abstraction must land now.** Local addressing funnels through `slot_addr` and
`val_addr`, which return a bare `u16`, across 87 call sites in `isel`. Each site formats
that address directly into an operand. Frame-relative access is not a different address, it
is a different *number of instructions*, so retrofitting it later means revisiting all 87
sites. Introducing the abstraction while those emitters are being written for a new
instruction set anyway costs approximately nothing; doing it afterwards is a large refactor
of a 5,000-line crate.

```rust
/// Where a local lives. v1 only ever constructs `Direct`.
enum Slot {
    /// Statically allocated: a direct file address.
    Direct(u16),
    /// Frame-relative, FSR2 + offset. Reserved for the later reentrancy phase.
    Frame(i8),
}
```

Emitters must ask a `Slot` for its operand and never see a raw address.

**The hook already exists.** `crates/callgraph/src/lib.rs` detects call cycles by DFS
colouring and currently panics on one. Adding recursion later means turning that panic into
"route these functions to the frame allocator." The whole-program analysis is already
written and tested.

### D-3: Device support as a Rust struct, two profiles

**Decision:** a `device` crate exporting a `Device` struct with the two families' memory
maps and capability flags, as compile-time constants. Not TOML, not generated from gputils
`.inc` files.

**Rationale.** ADR-004's device-description-as-data design is still unimplemented and the
PIC16F877A is hard-coded across `alloc`, `isel`, `banking` and `asm`. A second architecture
is the point at which that stops being optional, but the full ADR-004 file format plus a
loader plus schema tests is not what the port needs. Two structs behind one selector
removes the hard-coding, threads a device through the pipeline, and leaves ADR-004's file
format for when a third and fourth device actually arrive.

The capability flags are what let stages ask about behaviour rather than about part
numbers: `has_hardware_multiply`, `has_tblrd`, `access_bank`, `stack_depth`,
`interrupt_vectors`.

### D-4: Standard instruction set; `XINST` stays off

**Decision:** target the standard PIC18 instruction set. The extended instruction set
(`XINST` configuration bit) is not used in v1.

**Rationale.** `XINST` adds indexed literal offset addressing, which would make
frame-relative access a single instruction again and largely dissolve D-2's cost argument,
plus `ADDFSR`/`SUBFSR`/`PUSHL`. But it changes instruction semantics globally, it is off by
default and therefore the less-travelled path in tools and silicon errata, and enabling it
would add a second novel variable during a port. **Revisit if and when reentrancy becomes a
priority**, since that is the case where it pays.

### D-5: USB registers named, no USB stack

**Decision:** the PIC18F4550 device profile carries the full SFR table including the USB
registers (UCON, USTAT, UEPn, UADDR and the rest), so a user can write a driver in C without
magic addresses. A USB stack is out of scope.

**Rationale.** USB is a peripheral driver problem, not a compiler problem. The compiler's
obligation is correct code generation and honest register names. A stack belongs in a
library above the compiler, in `epic-hal` if anywhere.

---

## 3. Architecture

### Crate layout after the port

```
crates/
  ir/            unchanged   the IR data model
  irparse/       unchanged   LLVM IR text parser
  wholeprog/     unchanged   module merging
  callgraph/     unchanged   call graph, cycle detection, depth check
  legalize/      adapted     routine selection becomes device-aware
  alloc/         adapted     bank model comes from Device; emits Slot
  device/        NEW         the Device struct and the two profiles
  iselcore/      NEW         shared scaffolding: dominators, phi edges,
                             block labelling, Slot, slot key naming
  isel/          unchanged   the PIC14 backend
  isel-pic18/    NEW         the PIC18 backend
  banking/       adapted     PIC14 RP1:RP0 path kept; BSR path added
  peephole/      adapted     PIC14 PCLATH path kept; PIC18 pass is separate
  asm/           adapted     PIC14 encoder kept; PIC18 encoder added
  sim/           adapted     PIC14 core kept; PIC18 core added
  driver/        adapted     selects the pipeline by device
  fuzz/          adapted     device flag threaded through; harness unchanged
```

### The `Device` struct

Sketch, not final. Every constant is `[VERIFY]` against DS39632.

```rust
pub struct Device {
    pub name: &'static str,
    pub core: Core,                  // Pic14 | Pic18
    pub flash_words: u32,            // 16,384 for the 4550  [VERIFY]
    pub ram_banks: &'static [Bank],
    pub access_bank: Option<Range<u16>>,   // PIC18 only
    pub common_ram: Option<Range<u16>>,    // PIC14 only
    pub stack_depth: u8,             // 8 (PIC14) / 31 (PIC18)  [VERIFY]
    pub interrupt_vectors: &'static [u16], // [0x0004] / [0x0008, 0x0018]  [VERIFY]
    pub has_hardware_multiply: bool,
    pub has_tblrd: bool,
    pub sfrs: &'static [Sfr],        // name, address, bit fields
}
```

### Data flow

The ten-stage pipeline is unchanged in shape. Every stage boundary remains a diffable text
artifact, which is what makes a miscompile bisectable to a stage, and that property matters
more during a port than at any other time.

```
.c → clang → .ll → irparse → wholeprog → legalize → callgraph → alloc
                                                                 ↓
                            device selects: isel | isel-pic18 → banking
                                                                 ↓
                                              peephole → asm → .hex
```

The branch is at exactly one place, in the driver, and it selects a backend rather than
threading a boolean through the backend.

---

## 4. Phases

Nine phases, front-loaded on de-risking. Each has an acceptance criterion that is a test,
not a judgement.

| Phase | Deliverable | Acceptance |
|---|---|---|
| **P0** | `device` crate; de-hard-code the 877A from `alloc`, `isel`, `banking`, `asm`; introduce `Slot`. No PIC18 code. | The existing test suite still passes, unchanged. Pure refactor. |
| **P1** | PIC18 `asm` encoder and `sim` core. Hand-written `.asm` inputs only, no codegen. | `gpasm -p p18f4550` byte-for-byte HEX match, plus simulator tests per instruction group. |
| **P2** | Integer spine: `isel-pic18`, Access Bank + BSR banking, `Slot::Direct`. | `add.c`, `scalar.c`, `overlay.c`, `banked.c` |
| **P3** | **DONE** Pointers, arrays, structs via FSR0/1. | `ptr_probe.c`, `array.c`, `structs.c`, `banked_ptr.c` |
| **P4** | **DONE** `const` in flash via `TBLRD`. | `const_table.c`, `ptr_probe.c`; the 511-byte ceiling stops existing |
| **P5** | **DONE** Interrupts: single-vector compatibility mode (IPEN=0), one handler at `0x0008` (see the P5 note and ADR-013). | `interrupt_pic18.c`, `interrupt_gate_pic18.c`; `interrupt_mul.c` joins P6 (it needs `*`/`/`) |
| **P6** | **DONE** 32-bit `long`, and hardware `MUL` throughout. | `long.c`, `muldiv.c`, `interrupt_mul_pic18.c` |
| **P7** | **DONE** Soft-float: nine f32 routines via `MULWF`/`TBLRD`/`isr` save area. | `float.c` (out1=0x3F99999A, out2=0x41100000, out3=0x3EAAAAAB) |
| **P8** | Point the differential fuzzer at PIC18. | the seed corpora run clean |

**P3 note (2026-08-20):** landed per
[`docs/superpowers/plans/2026-08-20-pic18-port-p3.md`](superpowers/plans/2026-08-20-pic18-port-p3.md).
`ptr_probe.c` as originally listed bundles a RAM pointer with a `const`-flash
table read; since `TBLRD` is P4's job, P3 used a substitute
`ptr_probe_pic18.c` (RAM pointer only) and the original file's full parity
becomes a P4 acceptance addition once `TBLRD` lands.

**P4 note (2026-08-20):** landed per
[`docs/superpowers/plans/2026-08-20-pic18-port-p4.md`](superpowers/plans/2026-08-20-pic18-port-p4.md),
closing the P3 note: the ORIGINAL `ptr_probe.c` (RAM pointer + `const` read)
now runs on PIC18 via `TBLRD` (ADR-010). `const` reads are linear
byte-packed flash reads through `TBLPTR`/`TABLAT`; the PIC14 `RETLW`
chunk machinery (256-byte windows, 511-byte ceiling) is not ported, so the
ceiling stops existing. `const_table.c` (300 bytes) passes with its PIC14
expected value.

**P5 note (2026-08-20):** landed per
[`docs/superpowers/plans/2026-08-20-pic18-port-p5.md`](superpowers/plans/2026-08-20-pic18-port-p5.md).
The "interrupt priority" open question from §7 is settled by
[ADR-013](adr/ADR-013-pic18-interrupts.md): v1 targets the single-vector
compatibility mode (IPEN=0, one handler at `0x0008`, GIE-gated), the same
model PIC14's fixtures exercise; two-vector priority mode stays a
documented follow-up. The fixtures are PIC14's `interrupt.c`/`interrupt_gate.c`
with the SFR addresses changed (PORTB 0x06→0xF81, INTCON 0x0B→0xFF2);
`interrupt_mul.c` needs `*`/`/` and moves to P6. The shared ISR plumbing
(`Func.isr`, legalize duplication, alloc's disjoint ISR region) is reused
from PIC14 M13 unchanged.

**P6 note (2026-08-20):** landed per
[`docs/superpowers/plans/2026-08-20-pic18-port-p6.md`](superpowers/plans/2026-08-20-pic18-port-p6.md).
The i32 surface and the runtime routine recipes land together (ADR-014):
`long.c` (0x1634943A), `muldiv.c` (210), and P5's deferred
`interrupt_mul_pic18.c` (main + ISR both reach `__mul_u8`/`__udiv_u8`, the
`_isr` copies get disjoint frames) all pass on the `Pic18` simulator, with
a `gpasm -p p18f4550` byte-for-byte cross-check on the `long.c` asm. The
muls use hardware `MULWF` schoolbook partials (no shift-add loop); the
divmod/shift loops are branch-based with no single-GPR-bank constraint.

**P7 note (2026-08-20):** landed per
[`docs/superpowers/plans/2026-08-20-pic18-port-p7.md`](superpowers/plans/2026-08-20-pic18-port-p7.md).
The nine f32 soft-float routines land together (ADR-015): `float.c`
(out1=0x3F99999A, out2=0x41100000, out3=0x3EAAAAAB) passes on the `Pic18`
simulator with a `gpasm -p p18f4550` byte-for-byte cross-check. The
recipes are a 1:1 port of PIC14's verified ieee754 bodies with the
substitution table (RLF to RLCF, STATUS to 0xFD8, etc.); the frame rule is
every routine slot at `<=0x5F` (access-bank GPR, no MOVLB in skip windows).
**P0 deserves emphasis.** It is a pure refactor with the entire existing suite as its
oracle, and it is where `Slot` lands. If P0 is done well, every later phase is purely
additive and the PIC14 backend can never regress silently.

**P1 before P2 is deliberate.** De-risking the encoder and the simulator against `gpasm`
before any code generation exists means a P2 failure is never ambiguous between "chose the
wrong instruction" and "emitted the wrong bits." This is the same reasoning that put the
verification harness before the compiler originally.

**P5 gains scope over its PIC14 equivalent.** Two vectors with priority levels, gated by
the `IPEN` configuration bit, against PIC14's single vector.

---

## 5. Prerequisites and backlog interaction

### Must land before P2

**[#9, widen `Global.addr` to 16 bits](https://github.com/apojomovsky/epic-cc/issues/9)
is a hard blocker.** `ir::Global::addr` is `Option<u8>`. PIC18F4550 data memory spans
0x000-0x7FF and its SFRs sit at 0xF60-0xFFF `[VERIFY]`, so a `u8` cannot express either an
SFR address or a high-bank global. Nothing in P2 onwards works until this widens. It is
target-independent plumbing and should land first regardless of the port.

**[#10, constant folding for const-const operations](https://github.com/apojomovsky/epic-cc/issues/10).**
Small, lives in `ir`/`legalize`, target-independent, removes a class of panics. Every
backend benefits, and a new backend hits novel IR shapes more often than a settled one.

**[#14, extend differential fuzzing to signed and IR-level inputs](https://github.com/apojomovsky/epic-cc/issues/14).**
The strongest argument of the three. A port is exactly when a mechanical correctness net
earns its keep, and #14 already contemplates an **IR-level mode** feeding canonical IR
straight to the pipeline. That mode is worth more against a new backend than against a
settled one, because it exercises codegen without going through clang at all. Building it
after the port means porting blind.

### Obsoleted by the port, do not invest in them first

These are PIC14-specific. Doing them before the port means writing code the port deletes.

| Issue | Why the port obsoletes it |
|---|---|
| [#12](https://github.com/apojomovsky/epic-cc/issues/12) Pack functions into code pages | 20-bit `GOTO`/`CALL`. There are no pages. |
| [#17](https://github.com/apojomovsky/epic-cc/issues/17) Post-banking page-fit | Same. Possibly already resolved by PR #24; worth confirming and closing. |
| [#3](https://github.com/apojomovsky/epic-cc/issues/3) Const tables of i32/f32 | `TBLRD` reads flash directly; element width stops being special. |
| [#5](https://github.com/apojomovsky/epic-cc/issues/5) Const structs in flash | Same. |
| [#8](https://github.com/apojomovsky/epic-cc/issues/8) Const tables past 511 bytes | The 256-byte chunk chaining stops existing. |
| [#7](https://github.com/apojomovsky/epic-cc/issues/7) FSR globals inside bank windows | PIC18 FSRs are 12-bit and span the data space. |
| [#6](https://github.com/apojomovsky/epic-cc/issues/6) Bank-0 restriction on routine slots | Access Bank replaces the constraint. |
| [#13](https://github.com/apojomovsky/epic-cc/issues/13) Redundant `BANKSEL` sequences | Tied to RP1:RP0 and skip-sensitive sequences. Concept carries, code does not. Partly addressed by PR #25. |

They remain worth doing **for the PIC14 backend on its own merits**. The claim here is only
about ordering relative to the port.

### Either order

[#4](https://github.com/apojomovsky/epic-cc/issues/4) (dynamic-length memcpy) is
target-independent lowering and small. [#11](https://github.com/apojomovsky/epic-cc/issues/11)
(IEEE754 edge cases) will need the PIC18 float routines re-emitted regardless, but its
**edge-case corpus is reusable**, so building the corpus before P7 would give the new
routines a ready-made acceptance bar. Mild argument for doing #11's test half early.

---

## 6. Testing strategy

The port inherits the four verification layers rather than inventing any.

1. **Our own simulator**, extended with a PIC18 core. Deterministic, embeddable in
   `cargo test`.
2. **`gpasm` byte-for-byte cross-check**, against `-p p18f4550`. This is P1's entire
   acceptance criterion and the reason P1 precedes P2.
3. **The fifteen e2e acceptance programs**, recompiled for PIC18. They are the parity
   definition, phase by phase, in the P2-P7 table above.
4. **Differential fuzzing** against host clang, with the seed corpora.

Two additions specific to a port:

- **The PIC14 suite is a regression oracle for P0.** A pure refactor that keeps every
  existing test passing is the strongest possible evidence that the device abstraction did
  not change behaviour.
- **Cross-target differential.** The same C source compiled for both targets must produce
  the same observable results in the simulator. This catches target-independent stages
  regressing, and it is nearly free given both backends live in one workspace.

---

## 7. Risks and open questions

**Every `[VERIFY]` item above.** Memory map, bank layout, Access Bank extent, stack depth,
vector addresses, flash size. Confirm against DS39632 before hard-coding into `device`.

**PC addressing.** PIC18 instruction words are 16 bits and the program counter is
byte-oriented, while `GOTO`/`CALL` encode word addresses. The PIC14 simulator indexes its
program memory by word. Getting this boundary wrong is a whole class of P1 bugs, which is
precisely why P1 is gated on a byte-for-byte `gpasm` match rather than on inspection.

**Two-word instructions.** `GOTO`, `CALL`, `LFSR` and `MOVFF` occupy two program words. The
assembler's address computation, the page-free but still real branch-range checks, and the
simulator's fetch all have to agree. `[VERIFY]` the full list of two-word instructions.

**Interrupt priority.** With `IPEN` clear the core runs in a compatibility mode with a
single vector; with it set there are two. Which the compiler targets, and whether the
`__interrupt` attribute grows a priority argument, is **not settled** and is P5's first
design question.

**Soft-float duplication.** D-1 accepts writing the nine float routines twice. If that
proves worse than expected in practice, the fallback is to extract the algorithms over a
narrow emitter interface at P7, informed by two working implementations rather than by
speculation.

**The clang side is assumed unchanged.** `-target msp430` should remain the right datalayout
proxy for PIC18 (8-bit `char`, 16-bit `int`, byte alignment). `[VERIFY]` that PIC18 pointer
width does not argue for a different proxy, particularly for pointers into program memory.
