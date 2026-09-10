<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/epic-cc-logo-dark-mode.svg">
    <img src="docs/assets/epic-cc-logo-light-mode.svg" width="120" alt="Epic CC logo: a chip-temple inside a laurel wreath">
  </picture>
</p>

<h1 align="center">epic-cc</h1>

<p align="center"><em>A real C compiler for 8-bit PIC. clang front end, from-scratch backend, no Microchip toolchain.</em></p>

<p align="center">

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![CI](https://github.com/apojomovsky/epic-cc/actions/workflows/ci.yml/badge.svg)](https://github.com/apojomovsky/epic-cc/actions/workflows/ci.yml) [![Release](https://img.shields.io/github/v/release/apojomovsky/epic-cc)](https://github.com/apojomovsky/epic-cc/releases) [![target PIC16F877A](https://img.shields.io/badge/target-PIC16F877A%20%28PIC14%29-c0392b.svg)](docs/01-target-pic14.md) [![status: alpha](https://img.shields.io/badge/status-alpha-yellow.svg)](#status)

</p>

8-bit PIC still ships in enormous volume, and the only C toolchain for it has been
Microchip's closed-source XC8: capped optimization on the free tier, no modern
diagnostics, nothing to inspect when it goes wrong. `epic-cc` is a real compiler
instead: clang's front end for genuine C semantics, a from-scratch whole-program
PIC14 backend with nothing to license, and every stage checked against a
byte-for-byte oracle. It's the default toolchain behind
[epic-hal](https://github.com/apojomovsky/epic-hal)'s one-command PIC projects.

## Quickstart

Grab the latest release and compile something. No docker and no Microchip
download required: the bundle ships its own pinned clang.

```bash
gh release download -R apojomovsky/epic-cc --pattern '*x86_64-linux.zip'   # or *-windows.zip
unzip epic-cc-*-x86_64-linux.zip && cd epic-cc-*-x86_64-linux/
```

```console
$ cat add.c
volatile unsigned char in;
volatile unsigned char out;
void main(void) { out = in + 1; }

$ ./epic-cc --target p16f877a add.c -o add.hex && cat add.hex
:020000040000FA
:10000000012800308A0005206300831203132008B2
:0E001000A2002208013EA3002308A100080060
:00000001FF
```

No `gh` CLI? Download the zip straight from
[Releases](https://github.com/apojomovsky/epic-cc/releases/latest). Building
your own PIC project rather than testing the compiler? Start from
[epic-hal](https://github.com/apojomovsky/epic-hal) instead: its installer
downloads and wires up both.

Building from source (contributors, or you want the bleeding edge)? Everything
runs inside a pinned docker image, so nothing installs system-wide:

```bash
make image && make shell    # first build is slow: compiles clang from source
cargo test --workspace
```

See [`docs/09-build-environment.md`](docs/09-build-environment.md) for the
pinned versions and build-cache notes.

## What you get

- **Real C, real diagnostics.** clang parses and type-checks your source; you
  get clang's errors, not a home-grown parser's guesses.
- **Whole-program compiler, straight to `.hex`.** One invocation, no external
  assembler or linker: `epic-cc` owns every stage from IR to Intel HEX.
- **Built for one architecture, not retrofit onto one.** Every register write
  is cited to Microchip's datasheet; see [why that's the hard part](#under-the-hood).
- **Verified, not just tested.** Emitted assembly is cross-checked byte-for-byte
  against real `gpasm`, and a differential fuzzer runs every generated program
  against host clang on every commit.
- **Loud panics, never silent miscompiles.** Anything unsupported aborts with a
  specific message instead of emitting wrong code.

## Status

**Alpha.** The full integer, pointer, interrupt, `long`, and soft-float spine is
implemented and passing end-to-end; a fast test subset gates every commit.

| Feature | State |
|---|---|
| Core C89 control flow, non-recursive calls | ✅ |
| 8-bit and 16-bit integers, all comparisons | ✅ |
| Pointers, arrays, structs (`sret` / `byval`) | ✅ |
| `const` data in flash (`RETLW` tables, >256 bytes) | ✅ |
| Multi-bank RAM (`BANKSEL`) and multi-page flash (`PCLATH`) | ✅ |
| Interrupts, SFR access | ✅ |
| 32-bit `long`, IEEE-754 soft-float | ✅ |
| Unions | ⛔ not yet |
| Recursion | ⛔ by design: compile error, no escape hatch |

Devices ship as one file each (`crates/device/devices/*.toml`, generated from
Microchip's own device packs) and the registry grows continuously. See the
[device directory](crates/device/devices/) for the current list. Adding a
same-core part is a file, not a feature; see
[ADR-019](docs/adr/ADR-019-pic-variants-device-registry.md).

<a id="under-the-hood"></a>
<details>
<summary><strong>Under the hood</strong>: why this target is hard, how the pipeline works, and how correctness is verified</summary>

### Why this target is hard

Parsing C is solved. What's hard about a PIC14 compiler is **storage
allocation**, because the mid-range core breaks nearly every assumption a
conventional backend relies on:

| Constraint | Consequence |
|---|---|
| One accumulator (`W`), 35 instructions, no register file | Nothing to register-allocate: everything is `W` ⇄ memory. |
| 4 RAM banks selected via `RP1:RP0` | Every cross-bank access needs a `BANKSEL`; minimizing them is NP-hard. |
| 16 bytes of bank-independent common RAM | The only `BANKSEL`-free storage, half of what llvm-mos gets on 6502. |
| 8-level hardware call stack, not addressable | No stack frames, no recursion: locals are statically allocated and overlaid across the call graph. |
| Harvard architecture | `const` lives in program memory, reachable only through `RETLW` jump tables. |
| 368 B RAM / 8K words flash | Code size and RAM pressure are correctness concerns, not just quality ones. |

Full detail with datasheet cross-references: [`docs/01-target-pic14.md`](docs/01-target-pic14.md).

### Architecture: a ten-stage pipeline

Each stage is its own crate, and every stage boundary is a diffable text
artifact: a miscompile can be bisected to a stage before anyone reads code.

```mermaid
flowchart LR
    C[".c files"] --> CLANG["clang -S -emit-llvm"] --> LL[".ll text"]
    LL --> IRP["irparse"] --> WP["wholeprog"] --> LEG["legalize"]
    LEG --> CG["callgraph"] --> AL["alloc"] --> ISEL["isel"]
    ISEL --> BK["banking"] --> PH["peephole"] --> ASM["asm"] --> HEX[".hex"]
```

Three decisions shape everything:

- **clang out-of-process, not an LLVM backend.** We parse clang's `.ll` text
  output and go our own way: no libLLVM, no SelectionDAG, no TableGen. A prior
  attempt at this exact target with LLVM upstream (`llvm-pic`) was archived
  after 18 months without working `CALL`/`GOTO`. ([ADR-001](docs/03-decisions.md))
- **Whole-program compilation, down to HEX.** Locals can't live on a stack, so
  frames are statically overlaid using the *whole* call graph, which requires
  whole-program visibility by construction. ([ADR-002](docs/03-decisions.md))
- **`-target msp430` as a datalayout proxy**, for clang's ABI-independent type
  decisions (8-bit `char`, 16-bit `int`/pointers) without generating MSP430 code.

Repository layout:

```
crates/
  driver/ irparse/ ir/ wholeprog/ legalize/   # stages 1-4: front end -> legalized IR
  callgraph/ alloc/ isel/ banking/ peephole/   # stages 5-9: allocation, codegen, banking
  asm/                                         # stage 10: assembler -> Intel HEX
  sim/    fuzz/                                # PIC14 simulator, differential fuzzer
docs/     # design conversation, ADRs, milestone plans
```

### How correctness is verified

Four independent layers, all running in CI:

1. **Our own PIC14 simulator** ([`crates/sim`](crates/sim)): asserts on real
   register and RAM state, embeddable in `cargo test`.
2. **`gpasm` byte-for-byte cross-check**: our emitted assembly must match a
   real GNU PIC assembler's HEX output exactly.
3. **End-to-end acceptance programs** ([`crates/driver/tests`](crates/driver/tests)):
   real C through the full pipeline, run in the simulator, checked against
   hand-computed results.
4. **Differential fuzzing**: a seeded UB-free C generator compiles every
   program twice (epic-cc → sim, host clang → native) and diffs the checksums;
   mismatches auto-reduce to a minimal saved fixture.

### Known gaps

Deliberate and tracked, not surprises: diagnostics are panics rather than
user-facing errors; `BANKSEL` minimization is linear tracking, not the
published 2-approximation; overlay allocation is call-graph-based, not
interference-graph coloring; `.asm`/`.lst` output isn't exposed yet (only
`.hex` and `--map`); the XC8 differential oracle is designed but not wired
into the suite.

Full design conversation, ADRs, and per-milestone plans live in
[`docs/`](docs/); start with
[`docs/08-status-and-next-steps.md`](docs/08-status-and-next-steps.md).

</details>

## Documentation

- [`docs/00-charter.md`](docs/00-charter.md): goal, scope, non-goals
- [`docs/03-decisions.md`](docs/03-decisions.md): ADRs, with rejected alternatives
- [`docs/12-backend-design.md`](docs/12-backend-design.md): the approved backend spec
- [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`CLAUDE.md`](CLAUDE.md): conventions for contributors and agents

## Non-goals

- **Separate compilation**: whole-program is the point; overlay allocation needs the full call graph.
- **Debugger / COFF / ELF output**: HEX, listing, and map only.
- **Being an XC8 clone**: differential testing against XC8 is a verification technique, not a design target.
- **Reverse-engineering XC8**: prohibited by its license, and unnecessary.

## License

MIT, see [LICENSE](LICENSE). `gputils`/`gpsim` (GPL) are invoked as external
test-time processes only, never linked into the compiler. Microchip's
datasheets and XC8 are Microchip's property, used only as a black-box oracle,
and are not vendored here.
