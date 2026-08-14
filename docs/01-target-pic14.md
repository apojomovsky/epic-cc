# 01 — The target: PIC14 mid-range core / PIC16F877A

> **Verification note:** the figures below are working knowledge used to frame the design.
> Before any of them is hard-coded into the device description file, confirm against the
> **PIC16F87XA Data Sheet (DS39582)** and the **PICmicro Mid-Range MCU Family Reference
> Manual (DS33023)**. Both are free from Microchip; see [`07-references.md`](07-references.md).
> Items flagged **[VERIFY]** are the ones most worth checking first.

## Why this target is hard

Parsing C is a solved problem. The difficulty of a PIC14 compiler is concentrated almost
entirely in **storage allocation and instruction selection**, because the architecture
violates nearly every assumption a conventional compiler backend is built on.

### 1. One accumulator, no register file

There is a single working register `W`, and 35 instructions. There is no general-purpose
register file. Almost every operation is `W` ⇄ memory. Conventional register allocation
has nothing to allocate.

**Consequence:** we need an llvm-mos-style *imaginary register* file — a block of RAM
declared to behave like registers. See §"Common RAM" below.

### 2. Banked data memory

Data memory is split into **4 banks** selected by `RP1:RP0` in the `STATUS` register.
Every access to a variable outside the currently selected bank must be preceded by a bank
switch. PIC16F877A has **368 bytes** of general-purpose RAM **[VERIFY]**, laid out
approximately as **[VERIFY]**:

| Bank | GPR range | Bytes |
|---|---|---|
| 0 | 0x20–0x7F | 96 |
| 1 | 0xA0–0xEF | 80 |
| 2 | 0x110–0x16F | 96 |
| 3 | 0x190–0x1EF | 96 |

**Consequence:** a dataflow pass must track the bank state across the control-flow graph
and insert the minimum number of `BANKSEL`s. This problem is **NP-hard even when variables
are pre-assigned to banks**; a 2-approximation exists. See [`02-prior-art.md`](02-prior-art.md) §5.

### 3. Common RAM — our imaginary register file

Addresses **0x70–0x7F** (16 bytes) are mirrored into all four banks, so they are reachable
with **no `BANKSEL` at all** **[VERIFY]**. This is the direct analog of the 6502's zero
page, and it is where our imaginary registers must live.

It is **half the size** of the 32 bytes llvm-mos reserves on the 6502. That is expected to
be the single largest source of code-quality pain, and validating it is one of the four
questions the feasibility spike must answer ([`08-status-and-next-steps.md`](08-status-and-next-steps.md)).

The `llvm-pic` project reached the same conclusion independently — their calling-convention
wiki page opens with: *"Use special function registers 0x70 - 0x7F as preferred registers
because we don't need to switch bank to access (cheaper)."*

### 4. Eight-level hardware call stack, not addressable

The return-address stack is **8 entries deep, in hardware, and not mapped into data
memory**. You cannot read it, write it, or use it for anything but return addresses.

**Consequences, all severe:**

- **No recursion.** A recursive program cannot run. This must be a compile-time diagnostic,
  not a runtime failure.
- **No stack frames.** Locals cannot live on a stack because there is no addressable stack.
  Every local must be **statically allocated**.
- **Call depth is a hard resource.** The whole-program call graph must be checked against
  the 8-level limit, accounting for interrupt nesting.

**Consequence:** locals must be **overlaid** — functions that cannot be simultaneously live
(per the call graph) share the same RAM addresses. This is the core of the compiler and is
exactly what HI-TECH/XC8 market as "Omniscient Code Generation." llvm-mos calls the same
technique *static stack allocation* and measured these wins on the 6502:

| Operation | Dynamic stack (cycles) | Static allocation (cycles) |
|---|---|---|
| Function prologue | 10 | **0** |
| Function epilogue | 10 | **0** |
| Variable access | 8 | **4** |
| Array offset access | 18 | **5** |

### 5. Paged program memory

Program memory is addressed in **2K-word pages**, with the high bits supplied by `PCLATH`.
PIC16F877A has **8K × 14 bits** of flash = four pages **[VERIFY]**. Every `CALL` and `GOTO`
crossing a page boundary needs `PCLATH` management.

**Consequence:** a second dataflow pass, analogous to but distinct from banking, minimising
`PAGESEL`/`PCLATH` writes. A published heuristic exists; see [`02-prior-art.md`](02-prior-art.md) §5.

### 6. A single indirect pointer

Indirect addressing goes through exactly one register pair: `FSR` (8-bit offset) with `IRP`
supplying the ninth bit, read/written through the pseudo-register `INDF`.

This is materially **worse than the 6502**, which offers `(zp),Y` indirect-indexed
addressing through any zero-page pair. Pointer-heavy and array-heavy C will be the weakest
part of our generated code, and there is no clever way around it — it is an ISA limitation.

### 7. No hardware multiply

The mid-range core has **no multiply instruction** (unlike PIC18). Every multiply, divide,
and modulo — including the 16-bit ones implied by ordinary `int` arithmetic — becomes a
runtime library call.

**Consequence:** the runtime library is not optional garnish; it is on the critical path
for basic C semantics. Also note clang's optimizer will *normalize shifts and adds into
multiplies*, which we must then re-expand in the backend. The llvm-pic team flagged exactly
this in their notes from llvm-mos: *"Shifts and Adds are normalized as multiplies by
frontend. Need to be optimized by backend."*

### 8. Harvard architecture

Program memory and data memory are separate address spaces. `const` tables must live in
program memory, accessed either via `RETLW` jump tables (the classic HI-TECH approach) or
via flash self-read through `EECON` **[VERIFY]** — which is slow.

LLVM IR assumes a single flat address space. Reconciling this is a known, bounded cost
handled by an address-space attribute plus a dedicated lowering pass — but it is a problem
the 6502 never had, so **llvm-mos provides no prior art here**. This is the fourth question
the spike must answer.

## Summary of the hard problems, ranked

1. **Overlay allocation** over the whole-program call graph, with a separate region for the
   interrupt call tree (the actual core of the compiler)
2. **BANKSEL minimisation** — NP-hard, 2-approximation published
3. **Instruction selection** for a single-accumulator machine
4. **PAGESEL / PCLATH minimisation** — published heuristic
5. **Harvard `const` data** lowering
6. **Runtime library** — soft mul/div for 16/32-bit, soft-float
7. **Pointer codegen** through a single FSR/INDF — quality ceiling, not a correctness problem

## Instruction set

35 instructions. The `llvm-pic` project enumerated them while building their (abandoned)
instruction selector:

```
Byte-oriented:  ADDWF ANDWF CLRF CLRW COMF DECF DECFSZ INCF INCFSZ IORWF
                MOVF MOVWF NOP RLF RRF SUBWF SWAPF XORWF
Bit-oriented:   BCF BSF BTFSC BTFSS
Literal/control: ADDLW ANDLW CALL CLRWDT GOTO IORLW MOVLW RETFIE RETLW
                RETURN SLEEP SUBLW XORLW
```

`DECFSZ`, `INCFSZ`, `BTFSC`, and `BTFSS` are skip instructions — they conditionally skip
the *next* instruction. This is the only conditional control flow the core has, and it
shapes how compare-and-branch must be lowered.
