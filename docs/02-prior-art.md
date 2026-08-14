# 02 — Prior art survey

Survey conducted 2026-08-14. This is the evidence base for the decisions in
[`03-decisions.md`](03-decisions.md). Read this before proposing any change of approach —
several attractive-sounding routes have already been tried by others and have failed.

---

## 1. llvm-mos — the proof that the techniques work

[llvm-mos](https://github.com/llvm-mos/llvm-mos) is a working LLVM fork targeting the MOS
6502. It is the strongest positive prior art we have, and it **falsifies the naive claim
that LLVM cannot target accumulator machines with no addressable stack.**

What they shipped:

- clang **and** Rust front ends; C99/C++11 freestanding
- Full LLVM and **LLD ELF** backend
- CoreMark at **0.0813 CoreMark/s per MHz** on a 1 MHz 6502
- Their own assessment: *"On average, generates sorta okay code… On occasion, generates
  great code!"*

### Their two core techniques — both directly applicable to us

**Static stack allocation.** Whole-program call-graph analysis. Functions not part of a
cycle get statically allocated `.bss` frames; only genuinely recursive subgraphs fall back
to a slow dynamic software stack. Interrupt handlers are marked
`__attribute__((interrupt))` and analysed as a **separate root**, so any function reachable
from both the main tree and the interrupt tree is identified and handled explicitly.

Measured cycle wins are in [`01-target-pic14.md`](01-target-pic14.md) §4.

**Imaginary registers.** The 6502's zero page is register-like, so they reserve **32 bytes
of it as imaginary registers** (`rc0`…`rc31` as 8-bit, `rs0`…`rs15` as 16-bit pairs),
declared to LLVM as genuine physical registers. Without this, *"Greedy Regalloc: error: ran
out of registers during register allocation."* After codegen these lower to `__rcXX`
symbols resolved by linker script.

Their calling convention over those imaginary registers:

| Register | Role | Saver |
|---|---|---|
| RS0 | Stack pointer | Callee |
| A, X, Y, RS1–RS7 | Argument / return | Caller |
| RS8–RS9 | Temporaries | Caller |
| RS10–RS14 | Saved registers | Callee |
| RS15 | Frame pointer / saved | Callee |

**Our analog:** PIC14's common RAM at 0x70–0x7F. But only **16 bytes**, half what they had.

### The other optimizations they listed

IV Index Extraction · Zero Extension in LSR Addressing Modes · Logical Pseudo-Instruction
Set · Early G_SELECT Lowering · RMW RegClass Widening · Global NZ Flag Invariant · Light
Spilling in Greedy Regalloc · Target-Specific CSR Slots · Custom Output Formats ·
Post-RA-Pseudo-Expansion Register Scavenging · Opportunistic NZ Flag Optimization

### The cost, quantified — this is the decisive number

From their EuroLLVM 2022 "Upstream?" slide:

> **"Outside of our target, we have a 22,421 line diff from upstream."**
> — including *"Major surgery was done to Loop Strength Reduction."*

You do not write an LLVM backend for a machine like this. You **fork LLVM's
target-independent core** and own that rebase permanently.

### They explicitly pointed at PIC

Slide 3, "Why build a 6502 LLVM backend?", ends with:

> *"If 6502, why not PIC? Intel 8051? Z80?"*

**Sources:** [EuroLLVM 2022 slides (PDF)](https://llvm.org/devmtg/2022-05/slides/2022EuroLLVM-LLVM-MOS-6502Backend.pdf) ·
[llvm-mos.org wiki](https://llvm-mos.org/) (note: Cloudflare-protected, `WebFetch` gets 403;
the slides are the reliable source) · [GitHub](https://github.com/llvm-mos/llvm-mos)

---

## 2. llvm-pic — someone already tried our exact target, and failed

This is the most important negative result in the survey.

[llvm-pic/llvm-pic](https://github.com/llvm-pic/llvm-pic) built an LLVM backend with the
target name **`PICMid`** — mid-range PIC14, **our exact target**.

| Fact | Value |
|---|---|
| Team | 3 people (Hannes — frontend integration; Lenni — core; Thanh — low-level integration and testing) |
| Active | ~2024-03 → last `PICMid` commit 2024-11-09 |
| Archived | **2025-11-04**, read-only |
| Traction | 24 stars, 1 fork |
| Approach | Full LLVM fork, GlobalISel, TableGen |
| Support | Direct mentorship from llvm-mos devs (an entire wiki page of Q&A) |

### Their requirements were far below ours

Verbatim from their requirements page — the *entire* language scope they aimed at:

> 1. Generate PIC16F883-compatible machine code, flashable via MPLAB IPE v6.15
> 2. A subset of ANSI C
> 3. Function definitions
> 4. Integer addition/subtraction; bitwise OR, XOR, AND, COM; setting/clearing individual bits

No multiply. No pointers. No floats. No interrupts. No arrays.

### How far they got

Their instruction-implementation table (as of 2024-03-14) showed ~20 of 35 instructions
partially done — but the ones marked **unimplemented** include `CALL`, `GOTO`, `MOVF`,
`MOVWF`, `CLRF`, `NOP`, `BCF`, `BSF`, `RETFIE`, `CLRWDT`, `SLEEP`. That is to say: they had
arithmetic pseudo-instructions but **no working function calls or jumps**.

### The single most telling detail

Their wiki page **"Switching Register Banks" is a zero-byte file.** They never began work
on banking — the problem that, along with overlay allocation, *is* the PIC14 compiler.

Their last four commits are also diagnostic of the fork tax:

```
2024-11-09  fix: Breaking changes introduced upstream
2024-09-03  [PICMid] Stop using deprecated LLVM features
2024-09-02  fix: Breaking changes introduced with llvmorg-20-init
2024-08-31  chore(PICMid): Delete examples
```

Three of the last four commits are upstream-churn maintenance, not progress.

### Useful things salvaged from their wiki

- The instruction-set enumeration (reproduced in [`01-target-pic14.md`](01-target-pic14.md))
- The 0x70–0x7F common-RAM-as-registers insight, arrived at independently
- Their Q&A with the llvm-mos team, which is the source of several notes here — notably
  *"Shifts and Adds are normalized as multiplies by frontend. Need to be optimized by
  backend"* and *"More difficult to not have recursion in LLVM than to have recursion."*
- Their own MVP advice list, which reads as hard-won: *"Not start with Assembler /
  Linker" · "Emphasis on codegen" · "Stay out of TableGen" · "SelectionDAG heavily depends
  on this btw" · "Do not naively copy and paste from other Backends" · "No floating point"
  · "Next milestone: printf(\"%s\", \"Hello, World!\")"*

The wiki is a separate git repo and can be cloned even though the main repo is archived:

```bash
git clone --depth 1 https://github.com/llvm-pic/llvm-pic.wiki.git
```

---

## 3. The original LLVM PIC16 backend — removed from LLVM

Microchip contributed a PIC16 backend that shipped in LLVM up to **2.8** and was **dropped
in 2.9**.

From the removal commit (r116190):

> *"When/if it comes back, it will be largely a rewrite, so keeping the old codebase in
> tree isn't helping anyone."*

Community assessment: it *"started to bitrot quite fast, and no one from the community felt
brave enough to maintain it."* It was also reported to have been *written violating LLVM
guidelines, with authors refusing to address review comments.*

**Do not attempt to revive it.** The consistent advice from LLVM developers is to start
fresh.

**Source:** [PIC16 removal details — LLVM Discourse](https://discourse.llvm.org/t/pic16-removal-details/20754)

---

## 4. Existing PIC compilers

### SDCC pic14 port — unmaintained

The pic14 and pic16 ports are **unmaintained and do not pass their own regression tests**.
Users are explicitly warned off them. Additional known problems: not all required libraries
are built; device-header licensing issues led Debian to strip the non-free headers
([Debian #867136](https://bugs.debian.org/cgi-bin/bugreport.cgi?bug=867136)).

Architecturally it is also built around **per-file compilation**, which fights the
whole-program overlay allocation that is central to our design.

### XC8 / HI-TECH C — the incumbent, and what it actually is

**Important correction to a common misconception.** XC8 v2.x+ is marketed as clang-based.
Inspecting the local install at `/opt/microchip/xc8/v4.00/pic/bin/` shows:

```
aspic  aspic18  cgpic  cgpic18  clang  clist  cromwell  driver  driver18
dump  hexmate  hlink  libr
```

`clang` is a **front end only**. For mid-range PIC16 the actual code generator is
**`cgpic`** — the HI-TECH C lineage backend, a design of ~1990s vintage. So "reproduce
modern XC8 for a 16F877A" means reproducing a HI-TECH-era codegen, and **LLVM buys nothing
structurally there.**

XC8's marketing term for whole-program compilation is **Omniscient Code Generation (OCG)**:
it *"optimizes stack and register allocation across all code modules prior to generating
the object code… collects comprehensive data on every register, stack, pointer, object and
variable declaration across the entire program."* Claimed results: code footprint cut by up
to half, 10–15% of SRAM freed.

This is the same technique as llvm-mos's static stack allocation, and it is what we are
building.

### Others

CCS, mikroElektronika (mikroC), SourceBoost/BoostC — proprietary, closed, no published
internals. Not useful as prior art beyond confirming the market exists.

---

## 5. The two hard problems have published algorithms

This is a major de-risking finding: **we are not inventing the core optimization passes.**

### BANKSEL minimisation

[*Minimizing Bank Selection Instructions for Partitioned Memory Architectures*, CASES'06](https://cgi.cse.unsw.edu.au/~jingling/papers/cases06.pdf)

- Proves the problem **NP-hard even when variables are pre-assigned to banks**
- Gives a **2-approximation** via a rounding method
- Follow-up analysis: [*Analysis and approximation for bank selection instruction
  minimization on partitioned memory architecture*, J. Combinatorial Optimization](https://link.springer.com/article/10.1007/s10878-010-9365-z)
- Related: *Optimizing Bank Selection Instructions by Using Shared Memory* — uses a relation
  matrix of bank state transitions to detect redundant bank-selection code; prototyped
  specifically on **PIC 16F87X**, i.e. our exact family

### PAGESEL minimisation

[*A Heuristic Algorithm for Optimizing Page Selection Instructions*](https://arxiv.org/pdf/1008.0909)

Both are short papers describing precisely the passes we need. Muchnick chapters 8
(data-flow analysis) and 13 (redundancy elimination) supply the implementation machinery —
BANKSEL placement is partial-redundancy-elimination-shaped.

---

## 6. Tooling ecosystem

### gputils — actively maintained

**Correction to an earlier assumption in this project:** gputils is *not* abandoned.

- Stable release **1.5.2, 2025-10-23**
- Maintained by David Barnett, primarily supported by Molnár Károly
- `gpasm`, `gplink`, `gplib` — designed to be compatible with MPASM/MPLINK/MPLIB
- GPL

**Why this matters to us even though we write our own assembler:**
1. Its device `.inc` files are a ready-made source for our device description database
2. `gpasm` is a free **differential oracle** for our own assembler's output

**Source:** [gputils.sourceforge.io](https://gputils.sourceforge.io/)

### gpsim — mature PIC simulator

Version **0.32.1 (November 2023)**; supports all three PIC core families (12-bit, 14-bit,
16-bit). GPL. Useful as a cross-check oracle, though we plan our own simulator for speed,
determinism, and embeddability — see [`05-verification.md`](05-verification.md).

### Compiler testing tooling

- **C-Reduce / cvise** — automated test-case reduction. Regehr et al., PLDI'12: outputs
  average **>25× smaller** than delta debugging or other reducers. The user writes an
  oracle shell script describing the unwanted behaviour.
  [Paper](https://users.cs.utah.edu/~regehr/papers/pldi12-preprint.pdf)
- **YARPGen** — random C/C++ generator producing **UB-free** programs; found 220+ bugs in
  GCC/LLVM/ICC. Preferred over Csmith here because Csmith leans on 32-bit-`int`
  assumptions and UB-adjacent patterns that would waste time on a 16-bit-`int` target.
  [OOPSLA'20 paper](https://users.cs.utah.edu/~regehr/yarpgen-oopsla20.pdf)

---

## 7. What the survey concluded

The synthesis that drove [ADR-001](03-decisions.md):

> llvm-mos proves the **techniques** work. llvm-pic proves that on PIC14 specifically,
> embedding those techniques **inside LLVM's backend framework** consumed three people and
> eighteen months without reaching a working `CALL` instruction.

Therefore: take llvm-mos's two ideas — static stack allocation and imaginary registers —
and implement them in our own backend, where each is a few hundred lines of straightforward
code rather than a fight with GlobalISel, TableGen, and a 22,000-line upstream diff.
