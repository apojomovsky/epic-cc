# Changelog

All notable changes to this project are documented here, generated from
Conventional Commits. Dates are UTC.
## [Unreleased]

### Bug Fixes

- Scope release artifact download and move artifact actions off node20 (#379)
- Sweep PIC14 and PIC14E DFP packs, map baseline arch names (#380)
- Sweep baseline DFP pack, derive fsr_bank_bits, gate DCR to pic18 (#381)
- Keep wide-compare skip chains atomic on 0xFF folds (#384)
- Floor variadic va regions to one byte for va_start base (#400)

### Documentation

- Fix factual errors found by a real fact-check pass (#378)
- Retire runbook sections 2-6 in favor of add-device.sh (#385)
- Record P7 soft-float as documented non-goal (#386)
- Non-goal rationales plus landed-state table refresh (#388)
- Conclude printf putchar-sink probe as SDCC limitation (#390)
- Common_ram classification design for the 6 excluded PIC14 devices (#397)

### Features

- P6 32-bit long add/sub/shift/compare plus mul/div routines (#382)
- Freestanding malloc/free plus honest Tier-2 probe (#392)
- Math.h library and honest Tier-2 probe (#389)
- Onboard 15 catalog-selected high-tier devices (#394)
- Add p12f683 (PIC12F683) (#396)
- Implement ADR-034 single-region common_ram split (#399)
- PIC16F74 full interrupt support (docs/39 D-2) (#401)
- Pic-baseline differential fuzz gate and PicBaseline arm in run_pic (#402)
## [0.2.0] - 2026-09-10

### Bug Fixes

- Wire emit_commutative through the W-tracking cache (#218)
- Gen-device.py mis-generates PIC18 config and RAM layout (#231)
- Drop the hardcoded part name from the pic14 xtal_hz panic (#233)
- Derive the PIC18 access-bank boundary from device data (#234)
- Refuse to write pack = unknown in gen-device.py (#239)
- Reload the subtrahend in const-LHS sub fold (#242)
- Count __start init and module-asm words in page-0 base (#244)
- Disable fp-contract so float mul+add compiles (#281)
- Stop base apt-get changes from busting the clang-builder cache (#280)
- Isolate gputils/SDCC's from-source builds from unrelated dev deps (#293)
- Non-root dev image default, defense in depth (#294)
- Resolve PIC18 sim PANIC; document recursion (#296)
- Add ELA-aware parse_hex_pic14e for the PIC14E gate hexes (#307)
- Default the optional PIC14E config fields to erased values (#308)
- Lower i1 loads and stores as bytes (#310)
- Isolate local clang cache from docker system/builder prune (#312)
- Sweep the PIC18Fxxxx DFP pack, fix alias-table gaps (#361)
- Stop compat ISR W save from clobbering FSR0H's snapshot (#363)
- Stage indirect store values before the FSR setup (#367)
- Restore compat ISR SFRs before the retval backup (#369)
- Halve ini ROMSIZE for PIC18, it states bytes not words (#371)

### Documentation

- ADR-027, a bank-aware instruction scheduler pass (#219)
- Write docs/32-adding-a-device.md, the same-core device runbook (#235)
- PIC14E (Enhanced Mid-range) backend port design (#236)
- Land the phase 2-4 design doc as docs/34 (#241)
- Revise the port design per independent review (#243)
- Require a separate code review before takeoff (#254)
- Track PIC baseline core as a future backend candidate (#256)
- Record settled phase 2-4 scope, fix full -g mechanism (#260)
- Board-track every filed issue (#262)
- SDCC parity epic design (docs/35) (#271)
- PIC18 float-routine access-bank frame allocation (design) (#285)
- Robustness test strategy beyond differential fuzzing (docs/36) (#292)
- PIC baseline core port design (docs/37) (#321)
- Device onboarding hardening design (docs/38) (#332)
- State peripheral-blindness and four-core scope decisions (#341)
- Rewrite as a two-speed pitch instead of a design doc (#377)

### Features

- Add the crates/schedule skeleton as an identity transform (#221)
- Implement the phase-1 singleton-excursion swap (#222)
- Implement the phase-2 dead-W bundle hoist (#223)
- Add PIC18F2550 as a second PIC18 device (#232)
- Address-to-line table via --line-table (#240)
- PIC14E 193x device TOMLs and config hex-path fix (#245) (#255)
- PIC14E encoder and sim core (pic14e P1) (#261)
- Integer spine and BSR/MOVLB banking (pic14e P2) (#264)
- P3 pointers/arrays/structs via FSR0/1 + linear addressing (#272)
- Typed variable table via full -g debug metadata (#274)
- PIC14 debugger control surface: run_until, registers, memory (#275)
- SDCC 4.6.0 oracle + differential harness + baseline (P0) (#276)
- Support double as f32 on msp430 (PIC18 parity) (#277)
- P4 const in flash via RETLW (#278)
- Thread double through call/param/return/binop (#279)
- Interrupts with hardware context save (P5) (#283)
- P6 32-bit long and mul/div routines (#284)
- Gdbstub adapter plus ELF-DWARF sidecar as epic-cc-gdbserver (#282)
- Pin PIC18 float routines into the access-bank window; ship %f printf (#295)
- P7 soft-float f32 routines (#297)
- PIC14E differential fuzz gate (P8) (#298)
- PIC14E differential parity on 16F1938 (#302)
- Coverage-guided fuzz target for irparse parser (Lane B) (#311)
- Arbitration gate with sdcc-known-bugs.toml (docs/35 5.2) (#313)
- Valid comparison protocol, ratio gate, P0 closure (#319)
- Wire w_holds into const-table index and i8 add stores (#320)
- SDCC regression-suite pass rate as nightly context (Tier 3) (#322)
- Pic-baseline core variant and p12f509 TOML, firewall-only (#340)
- Real %f, EEPROM and const-pointer probes with oracle hardening (#353)
- PIC8 flash-generation part catalog with popularity tiers (#355)
- Gen-device.py --sweep breadth-proofing mode (#358)
- PIC baseline P1 encoder and sim core (#359)
- Two-vector priority interrupts (#360)
- Add-device.sh one-command device onboarding wrapper (#364)
- Integer spine with D-2 FSR bank-bit reassertion (#365)
- P3 pointers, arrays, structs via FSR flat addressing (#368)
- Per-context float frames with banked recipes (#370)
- P4 const in flash via RETLW, 256-word ceiling (#374)
- Add PIC16F628A and scattered config mask support (#373)

### Miscellaneous

- Skip build+test loop on doc-only PRs (#263)
- Make image idempotent to skip the --load re-export (#315)
- Production-ready src comments without planning-stage prose (#354)
- Ruff auto-linter plus clean python baseline (#376)

### Testing

- Pin bit-fields and unions with e2e tests (PIC14 parity) (#300)
- Lane D idempotence, fixpoint, and determinism checks (#288) (#305)
- Compile-time bound for adversarial BANKSEL dataflow (Lane G) (#306)
- Resource-exhaustion boundary tests (Lane F) (#309)
- Pin volatile store ordering and count (docs/36 lane E) (#316)
- Lane C mutants-killing tests and triage checklist (#317)
## [0.1.0] - 2026-09-03

### Bug Fixes

- 256-align single-entry const tables that cross their window (#141)
- Correct the size report's bank counts and ISR fixed region (#157)
- Measure the const-section start at the assembler's final position (#159)
- Dispatch folded GEP and ptrtoint in phi and binop operands (#162)
- Ship a freestanding stdlib.h carrying size_t (#167)
- Resolve globals whose types contain unions (#169)
- Pack PIC18 structs to the XC8 byte-aligned record layout (#170)
- Handle local array alloca [N x T] via ty_size_align (#177)
- Lower llvm.fshl/fshr.i8/i16/i32 (LFSR rotate) (#178)
- Materialize const string literals for pointer call args (#179)
- Restore master red for cb_dispatch, cc2 and str_literal (#181)
- Handle i8 to i1 trunc via masked byte copy (#180)
- Overlay locals by liveness within a frame (#184)
- Seed non-folding pointer selects as indirect slots (#185)
- Pass runtime pointer call args and materialize data-global addresses (#188)
- Lower i64 aggregate struct copies as byte copies (#189)
- Land direct writes to banked SFR windows at the physical address (#190)
- Unblock full-example codegen (inttoptr comma, load-ptr seeding) (#192)
- Unblock the epic-sdcard/epic-settings PIC18 slice (isel-pic18 gaps) (#194)
- Whole-program IR cleanup closes the epic-encoder codegen-density gap (#198)
- Base need_stdio/need_string on clang's dependency output, not raw source text (#199)
- Always-inline single-call-site functions into ordinary callers (#208)
- Ship opt in the release bundles (#212)
- Authenticate the epic-tasks checkout for prose lint (#216)
- Elide a redundant store+immediate-reload of an SSA value in W (#215)

### Documentation

- Record the epic-math C-path measurement decision (#146)
- Drop pic-variants slice 2 plan, shipped in ci.yml and ADR-019
- Record the epic-serial put API decision (#91) (#150)
- Rename PlatformIO platform to epic8 in doc 31 (#158)
- Record the HAL-3 close-out and resolve the 877A RAM headroom question (#171)
- Record the epic-hal#97 literal printf shim and staged put_str (#174)
- Correct the XC8 optimization-tier caveat (#202)

### Features

- Support cross-context stored callbacks (#142)
- Size and map reporting (#156)
- Predefine the XC8 toolchain and part macros (#168)
- Source-accurate C locations for backend panics (#176)
- Decode const structs with function-pointer fields (#187)
- Variadic printf/stdio path (epic-cc#131) (#195)
- Elide a label's redundant BANKSEL when every path agrees (#213)

### Miscellaneous

- Remove root scratch asm dumps and CLAUDE.md placeholder
- Checkout the repo in the publish job (#182)

### Other

- Hal-pic16 slice e2e; arity-filter indirect calls; post-banking const window

* fix(isel): post-banking const window; arity-filter indirect calls

* fix(isel): check the const-section pin at the post-banking start (#153)

### Testing

- E2e for arity-filtered indirect call sites (#186)
- Pin the config table crossing its 256-byte window (#191)
- Add a flash/RAM size-regression suite (#201)
## [0.0.3] - 2026-08-25

### Miscellaneous

- Checkout the repo before creating the GitHub release (#140)
## [0.0.2] - 2026-08-25

### Miscellaneous

- Fetch clang license from source tag for the Windows bundle (#139)
## [0.0.1] - 2026-08-25

### Bug Fixes

- Honor PCLATH paging and INDF aliasing in bit ops
- Strip operand commas and carry global types
- Validate instruction width is i8
- Materialize constants instead of file-register reads
- Assert file register range
- Size global addresses by type width
- Guard i16 slots against the common-RAM boundary
- Scope tmp labels to the module and reserve the icmp scratch
- Reserve retval bytes in slot allocation
- Overlap-test retval and scratch reservations
- Close final-review minors (phi-cycle panic, layout guard, docs, token validation)
- Allow literal-immediate operands beyond bank 0
- Tolerate fn lines in callgraph input; drop vacuous assertion
- Apply IRP base for FSR in 0x80-0xFF
- Overlay bases on callers' physical frame ends
- Derive frame ends from placed locals
- Bank bit ops on GPRs
- Correct bank-3 base and reset bank at branch targets
- Guard array global size against overflow
- Guard empty const tables and align globals to two bytes
- Panic on sext of a constant operand
- Loud panics, struct-size assert, volatile memcpy
- Correct scaled multi-term FSR sums
- Round-trip scalar param widths; isel sret assert
- Panic on unimplemented runtime routines
- Sdiv sign xor and routine bank-0 bound
- Panic on unimplemented shift routines
- Enforce const-table window fit and label namespaces
- Align 256-byte const tables
- Decode backslash escapes in const literals
- Anchor exact-boundary functions and pin the const-table section
- Panic loudly on backward .org; pin CALL reset and overlap doc
- Const-operand icmp coverage and docs
- Panic on unimplemented i32 routines
- Skip llvm bookkeeping globals
- Flag-safe isr epilogue and retval save
- Guard the 8-byte isr save area
- Save the scratch byte in the isr prologue
- Panic when the isr context reaches main
- Explicit-width typedefs pin u32 semantics
- Frame-budget safety margin; isel negative-const masking
- Const-lhs sub test and zext i1 regression
- Phi-copy edge emission, poison call args, frame-budget recalibration
- Phi-copy ordering for separate-latch back edges
- Reduce() takes the fresh failure message; all-path fixture cleanup
- Drop dead code; isel negative-const test
- Panic loudly on soft-float routine names
- Cmp zero test covers the exponent lsb
- Decode f64-promoted float constants
- Mul low-part carry and add subtract rounding
- F64-promoted constants at untyped sites
- Wrap-correct INCFSZ borrow folds in i16 cmp and const-LHS sub chains (#20)
- Duplicate runtime routines for the interrupt context (#22)
- Verify post-banking page fit on the final layout (#24)
- Numeric LOW/HIGH/PAGE operands; skip BANKSELs in bank-0-only programs (#25)
- Widen Global.addr to u16 (#28)
- Fold const-const binops and comparisons (#30)
- Reclaim redundant BANKSELs; recognize bit-number bank ops (#13) (#31)
- Bin-pack globals across bank windows when sequential placement fails (#41)
- Decode named-struct globals and GEP field offsets (#78)
- Lower switch to icmp chain instead of default branch (#80)
- Resolve banked GPR from the device map, not a fixed window (#100)
- Start PIC14 banks 2 and 3 at the real GPR boundary (#101)
- Handle p16f887 ANSEL and isel gaps for 887 smoke (#107)
- Restore make compile and unbreak grep on irparse (#110)
- Per-worktree cargo target cache (#111)
- Select bank 0 for non-mirrored SFR operands (#122)
- Runtime-indexed const table reads through ccp_sel pointer selects (#124)
- Split common_ram into access_bank and fixed_retval (#129)
- Guard against escaped newlines in PR bodies (#130)

### Documentation

- Capture PIC14 compiler design, prior-art survey, and handoff notes
- Record interim spike findings on IR surface and storage pressure
- Record completed feasibility spike findings
- Mark ten-stage pipeline as approved
- Record pointer/const spike findings
- Add approved backend design spec
- Add phase 1 verification harness implementation plan
- Add integer spine milestone 1 pipeline skeleton plan
- Record final-review fix report
- Add integer spine milestone 2 plan
- Direct isel calls task to shared slot map refactor
- Add integer spine milestone 3 overlay plan
- Add integer spine milestone 4 banking plan
- Add phase 3 pointers/const milestone 5 plan
- Add integer spine milestone 6 scalar surface plan
- Add integer spine milestone 7 structs plan
- Add integer spine milestone 8 mul/div runtime plan
- Reword stale panics and fix comments
- Add integer spine milestone 9 multi-bank FSR plan
- Add integer spine milestone 10 pclath tables plan
- Fix stale comment and complete panic message
- Add integer spine milestone 11 multi-page plan
- Add integer spine milestone 12 long plan
- Correct expected long e2e value
- Add phase 4 interrupts and sfr milestone plan
- Add phase 6 random testing plan
- Add phase 7 soft-float plan
- Rewrite the README around the implemented architecture (#19)
- Add the PIC18 port design (#27)
- PIC18 port P3 implementation plan (pointers, arrays, structs) (#44)
- Point agents at the epic-tasks board for picking up work (#77)
- Refresh stale status doc and sweep tracked plan files off master (#79)
- File-per-device TOML + canonical-per-core CI design and ADR-019 (#87)
- Align ADR-019 and the spec with the shipped registry (#98)
- Record the 2026-08-24 epic amendments and refresh the status map (#120)
- State default task base is latest origin/master (#123)

### Features

- Decode Intel HEX into 14-bit words
- Core state and byte-oriented instructions
- Bit, literal, and control instructions
- INDF/FSR and PCL/PCLATH indirect addressing
- Define canonical IR text format with round-trip
- Parse LLVM IR text into canonical IR
- Single-module merge validation
- Type-width validation boundary
- Call graph and stack-depth check boundary
- Assign bank-0 addresses to globals
- Straight-line 8-bit instruction selection
- Bank-0 validation boundary
- Pass-through boundary
- Two-pass assembler to Intel HEX
- Chain the ten-stage pipeline end-to-end
- Control-flow, call, and cast instruction variants
- Parse control flow, calls, and casts
- Real edges, recursion detection, depth check
- 16-bit arithmetic, casts, and phi elimination
- Branches, compares, and select
- Calls and returns with arg/ret slots
- Emit parseable edge list
- Overlay local frames from the call graph
- Consume the complete overlay address map
- Bank-aware memory model
- Assign addresses across all four banks
- Insert BANKSEL across banks
- Gep instruction and sized globals
- Parse getelementptr and array globals
- Size arrays and keep const globals out of RAM
- Pointers via FSR/INDF and const tables via RETLW
- Resolve LOW/HIGH label operands
- All icmp predicates and sext
- Sub, or, xor and i8 and
- All comparison predicates
- Sign extension
- Struct types, alloca, memcpy and reworked gep
- Size alloca, byval and sret slots
- Pointer bases, chains, indirect and memcpy
- Byval and sret call abi
- Mul, div, rem, shift binops and freeze
- Lower mul, div, rem and shifts to runtime calls
- Mul, div and rem runtime routines
- Shift lowerings (inline const, runtime variable)
- Multi-bank FSR via IRP
- Sret targets in any bank via IRP
- 16-bit const table sizes
- Pclath const readers and page-0 bound
- Resolve PAGE() and bound to device flash
- Multi-page functions with pclath call discipline
- Elide redundant pclath writes
- Skip same-page pclath restores
- 32-bit type
- 32-bit arithmetic, compares, casts and shifts
- 32-bit mul, div, rem and shift routines
- Interrupt marker and inttoptr pointers
- Interrupt entry, prologue and sfr access
- Interrupt fire hook
- Duplicate interrupt-shared functions
- Encode swapf and retfie
- Seeded generator and differential harness
- Full generation surface and seed corpus
- Greedy differential reducer
- Float type and instructions
- Lower float ops to runtime calls
- Soft-float runtime routines
- Model GIE and interrupt enables; fix the return address (#23)
- Signed and IR-level differential fuzzing (#14) (#29)
- P0 — device crate and de-hardcode the 877A from alloc/isel/banking/asm (#32)
- Bin-pack functions into code pages (#12) (#33)
- P1 — PIC18 encoder and simulator core (#34)
- Const tables of i16/i32/float elements (issue #3) (#37)
- P2 — PIC18 integer-spine codegen (#38)
- Const tables beyond 511 bytes (issue #8) (#39)
- Dynamic-length memcpy (issue #4) (#40)
- Support const structs in flash (issue #5) (#42)
- Distribution — bundled clang, docker toolchain, release pipeline (#43)
- Lift bank-0 routine-slot restriction (#6) (#47)
- P3 — pointers, arrays, structs (issue #26) (#48)
- P4 - const via TBLRD (issue #26) (#50)
- Multi-file compilation via llvm-link (#51)
- Silicon-real codegen — EPIC_AT, EPIC_CONFIG, EPIC_FOSC_HZ (#56)
- P5 - interrupts (single-vector compat mode) (#52)
- P6 - long + hardware MUL (#53)
- P7 - soft-float via PIC18 recipes (#54)
- P8 - fuzz gate (device-threaded differential runner) (#58)
- Inline assembly — naked, module asm, opaque blocks (rungs 1-3) (#60)
- Freestanding libc subset (stdint, stdbool, stddef, string) (#61)
- Inline assembly rung 4 — memory operands (*m) (#65)
- Implement memchr/strchr/strrchr/strstr (#62) (#69)
- Unblock HAL C idioms for epic-cc variant (#70)
- Unified takeoff and worktree workflow (#81)
- File-per-device TOML registry with build.rs codegen and --target (#89)
- Add PIC16F887 as file-per-device exemplar (#90)
- Validate pic14e data, enforce fuse mask shape, expose sfrs (#96)
- Resolve device names in every spelling the toolchain uses (#97)
- Support epic-cc HAL build backend for HAL-2 (#88)
- Add DFP -> TOML generator with ATDF ingestion (#103)
- Provenance stanza and an always-on gputils cross-check (#106)
- Support calls through a function pointer (#128)
- Lint gate replaces the PROSE=1 attestation (#134)
- Lower inttoptr on a runtime address via slot-indirect (#132)
- Lower smax/smin/abs intrinsics and i1 immargs for #133 (#136)
- Publish rolling per-commit bundles for downstream CI (#135)

### Miscellaneous

- Add nix flake dev shell with pinned clang 20.1.8 and vendor layout
- Scaffold cargo workspace and pic14-sim crate
- Commit workspace lockfile
- Update lockfile for ir crate
- Lock legalize dev-dependency for isel
- Run the workspace test suite on every push/PR
- Add the MIT license (#21)
- Ignore agent worktree dirs (#36)
- Docker-first dev tooling, AGENTS.md, git hooks (#45)
- Takeoff ritual, ADR convention, em-dash hygiene (#46)
- Prose hygiene guards for the takeoff ritual (#49)
- Diff-scoped prose review gate for comments and docs (#55)
- Eliminate workspace compiler warnings (#57)
- Gate compiler warnings in pre-commit and pre-pr-check (#59)
- Rustfmt the workspace (#68)
- Stratify CI into canonical per core and per-device lightweight (#102)
- Build pic16f88x-hal for the 887 under epic-cc (#113)
- Refuse force pushes unless the human approves (#115)
- Add make bootstrap and make doctor for first-time setup (#116)

### Other

- Revert "docs: record final-review fix report"

This reverts commit 9d1bc0de1be82d0e129819859f0dbb0da51f4222.
- Handle IEEE754 edge cases in soft-float f32 arithmetic (#35)
- PIC18 4550 HAL slice smoke (blink + epic-tick)

* fix(sim): track :04 extended addresses in parse_hex_pic18

A config-region EPIC_CONFIG build emits a :04 extended-linear-address
record for the fuse base (0x300000 on the 4550) followed by the config
data records at low-16 address 0x0000. parse_hex_pic18 ignored :04, so
those data records aliased flash word 0 and clobbered the reset vector.

Track the extended upper address and drop data records that land
outside the 0x4000-word flash window (config words are not flash).

* test(driver): PIC18 4550 HAL slice smoke (blink + epic-tick)

The amendment recorded in docs/31 section 4: once CC-1 and CC-3 landed,
run a thin PIC18 smoke slice so the PIC18 path cannot drift silently
while PIC14 integration proceeds. Two programs compiled through the real
epic-cc binary (--target 18F4550, with the config-word TU exercising
CC-3) and run on the Pic18 simulator:

  - blink: toggles RB0 on INTCON<TMR0IF>, driving the HAL GPIO/Timer0
    drivers;
  - epic-tick: the 1 ms Timer2 timebase, delaying 10 ms then 5 ms and
    reporting the elapsed counts.

The simulator has no timer hardware, so simulated time is driven the
same way the 887 blink smoke does: the test asserts the timer flag
registers and the poll loop advances the observable counter.

Vendored HAL sources live in tests/fixtures/hal-pic18/ (the real
pic18fxx5x-hal sources adapted for the epic-cc backend, mirroring how
the 887 HAL's epic-cc variants work around open backend gaps). The e2e
asserts the emitted HEX matches committed golden .hex, so a backend
change that alters the codegen fails the gate. ci-test.sh picks the
test up automatically via the driver crate.

Open gaps filed as epic-cc#125 (i64 aggregate struct copies) and
epic-cc#126 (i8-to-i1 bool trunc); runtime SFR addressing is epic-cc#117
and function-pointer calls are epic-cc#73. The slice compiles around
them exactly as the 887 HAL's epic-cc variants do.

* fix(driver): drop golden-hex comparison in HAL slice e2e

The committed golden *.hex referenced by the e2e is gitignored
(.gitignore *.hex), so it was never in the tree and the test panicked
reading it in CI (only four asm_*.hex fixtures are force-tracked; the
convention is to compile the .c fixture and assert simulator behavior).
The sim assertions already prove both programs run correctly, so the
golden-byte gate is redundant and its absence was a real CI break. (#127)

### Refactor

- Extract clang invocation into driver::clang helper (#82)

### Testing

- Cross-check simulator against gpasm
- Cross-check our assembler against gpasm
- Probe compiles, assembles, and runs correctly
- Overlay sibling frames share RAM and run correctly
- Cross-check overlay program against gpasm
- Multi-bank program compiles and runs with BANKSEL
- Pointers, arrays, and const tables run correctly
- Ordinary scalar C compiles and runs correctly
- Structs compile and run correctly
- Pin all 16 runtime routine mappings
- Mul, div, mod and shifts compile and run correctly
- Multi-bank FSR arrays compile and run correctly
- Drop duplicated statement in banked_ptr fixture
- Const tables past 256 bytes compile and run correctly
- Document elision as defensive pass
- Multi-page programs compile and run correctly
- 32-bit long compiles and runs correctly
- Interrupts and sfr access compile and run correctly
- Corpus gate counts, coverage table, panic-catching demonstration
- Float compiles and runs correctly
- Float differential corpus
- Re-capture float gpasm fixture
