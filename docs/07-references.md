# 07 — References

## Books — held locally

In `vendor/books/` — **gitignored, copyrighted, never commit them.** See
[`06-environment.md`](06-environment.md) for reading instructions and the Muchnick NFKC
gotcha.

| Book | File | Pages | Role |
|---|---|---|---|
| **Muchnick, _Advanced Compiler Design and Implementation_** (Morgan Kaufmann, 1997, ISBN 9781558603202) | `muchnick-advanced-compiler-design-1997.pdf` | 887 | Primary reference. Dataflow analysis, redundancy elimination, register allocation, interprocedural analysis — i.e. BANKSEL/PAGESEL placement and overlay allocation |
| **Fraser & Hanson, _A Retargetable C Compiler: Design and Implementation_ (lcc)** (Addison-Wesley, 1995, ISBN 9780805316704) | `fraser-hanson-retargetable-c-compiler-lcc-1995.pdf` | 578 | Complete burg-style tree-pattern code generators as working source. The isel technique for PIC14. Also the fallback reference if we ever drop clang for our own front end |

## Books — worth requesting if needed

The user has a large inherited technical library and responds well to specific titles.

- **Cooper & Torczon, _Engineering a Compiler_** (2nd/3rd ed) — best modern treatment of
  instruction selection, scheduling, register allocation
- **Appel, _Modern Compiler Implementation in C_** (or ML/Java — same book) — maximal-munch
  isel, graph-colouring allocation
- **Bob Morgan, _Building an Optimizing Compiler_** — SSA-based passes. Nice-to-have, not
  load-bearing

## Books deliberately NOT requested

- **Aho/Lam/Sethi/Ullman (Dragon Book)** — ~80% front end, and clang is our front end
- **Allen & Kennedy, _Optimizing Compilers for Modern Architectures_** — loop/parallelism
  focused; wrong end of the problem
- **Hennessy & Patterson** — architecture, not compilers
- **Predko, _Programming and Customizing PICmicro Microcontrollers_** — hobbyist level, no
  compiler content

## Papers — directly implementable

| Paper | Use |
|---|---|
| [*Minimizing Bank Selection Instructions for Partitioned Memory Architectures*, CASES'06](https://cgi.cse.unsw.edu.au/~jingling/papers/cases06.pdf) | **BANKSEL minimisation.** Proves NP-hard even with variables pre-assigned to banks; gives a 2-approximation by rounding |
| [*Analysis and approximation for bank selection instruction minimization*, J. Comb. Optim.](https://link.springer.com/article/10.1007/s10878-010-9365-z) | Follow-up analysis to the above |
| *Optimizing Bank Selection Instructions by Using Shared Memory* | Relation-matrix approach to detecting redundant bank-selection code; **prototyped on PIC 16F87X**, our exact family |
| [*A Heuristic Algorithm for Optimizing Page Selection Instructions*](https://arxiv.org/pdf/1008.0909) | **PAGESEL / PCLATH minimisation** |
| [Regehr et al., *Test-Case Reduction for C Compiler Bugs*, PLDI'12](https://users.cs.utah.edu/~regehr/papers/pldi12-preprint.pdf) | C-Reduce. Outputs >25× smaller than delta debugging |
| [Livinskii et al., *Random Testing for C and C++ Compilers with YARPGen*, OOPSLA'20](https://users.cs.utah.edu/~regehr/yarpgen-oopsla20.pdf) | UB-free random program generation |
| [Hjort Blindell, *Instruction Selection: Principles, Methods, and Applications*](https://kth.diva-portal.org/smash/get/diva2:951540/FULLTEXT01.pdf) | Free PDF from KTH. Survey of macro expansion / tree covering / DAG covering / graph covering |

## Prior implementations

| Project | Link | Relevance |
|---|---|---|
| **llvm-mos** | [GitHub](https://github.com/llvm-mos/llvm-mos) · [EuroLLVM 2022 slides](https://llvm.org/devmtg/2022-05/slides/2022EuroLLVM-LLVM-MOS-6502Backend.pdf) · [wiki](https://llvm-mos.org/) (Cloudflare-blocked) | The techniques we are copying: static stack allocation, imaginary registers |
| **llvm-pic** | [GitHub (archived)](https://github.com/llvm-pic/llvm-pic) · wiki: `git clone https://github.com/llvm-pic/llvm-pic.wiki.git` | Failed attempt at our exact target. Read the post-mortem in [`02-prior-art.md`](02-prior-art.md) §2 |
| **LLVM PIC16 backend** | [Removal discussion](https://discourse.llvm.org/t/pic16-removal-details/20754) | Removed in LLVM 2.9. Do not revive |
| **SDCC** | [sourceforge.net/p/sdcc](https://sourceforge.net/p/sdcc/) | pic14 port unmaintained, fails own regression tests |
| **gputils** | [gputils.sourceforge.io](https://gputils.sourceforge.io/) | v1.5.2 (2025-10-23), maintained. Device `.inc` files + `gpasm` cross-check oracle. **GPL** |
| **gpsim** | [gpsim.sourceforge.net](https://gpsim.sourceforge.net/) | v0.32.1 (2023-11). Independent simulator oracle. **GPL** |

## Microchip documentation — free, download as needed

- **PIC16F87XA Data Sheet (DS39582)** — the 877A specifics. Needed to confirm every
  **[VERIFY]** item in [`01-target-pic14.md`](01-target-pic14.md)
- **PICmicro Mid-Range MCU Family Reference Manual (DS33023)** — authoritative architecture
  reference for the PIC14 core
- **MPLAB XC8 C Compiler User's Guide** ([DS52053B mirror](https://ww1.microchip.com/downloads/en/DeviceDoc/52053B.pdf)) —
  the best *public* description of how a working PIC C compiler makes ABI, memory, and
  pointer-scoping decisions. Read the memory-allocation and ABI chapters.
- **MPASM Reference (DS33014L)** — assembler directives and syntax we should stay
  compatible with
- **PIC instruction listings** — [Wikipedia](https://en.wikipedia.org/wiki/PIC_instruction_listings)
  is a convenient cross-check on encodings
- **INHX8M format** — [description](https://www.lucidtechnologies.info/inhx8m.htm), the
  Intel HEX variant PIC tools expect

## Community resources

- [PICList routines](http://www.piclist.com/techref/microchip/routines.htm) — hand-written
  PIC assembly idioms; useful as a quality bar for our codegen and as a source of
  soft-arithmetic routines
