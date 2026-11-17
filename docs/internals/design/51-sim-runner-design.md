# 51: Simulator runner in the release bundle (epic-cc#1293)

> **Approval status:** design only, pending human approval. No code
> changes land with this document. The ticket is a product decision
> (what the release ships), so sections 2 and 3 record the recommended
> choices and the rejected alternatives for the approver to confirm.
> The CLI in section 3 is a new public interface: once approved and
> shipped, changing its flags, output lines, or exit codes is a
> breaking change under the repository commit rules.

**Goal:** a hobbyist who downloads only the release zip can run a
compiled program in the simulator, with no epic-hal checkout, no
epic-cc checkout, no absolute symlink, and no dev image. The
getting-started tutorial in epic-docs then reads: unzip, compile,
run `epic-cc-sim blink.hex`, read the output.

**Definition of done (for the follow-up implementation, not this
doc):** both release zips (Linux and Windows) carry the runner, the
bundle check fails without it, a smoke run of the shipped binary on
a fixture HEX passes on each platform, and the tutorial flow above
works from a clean directory containing only the unzipped bundle.

---

## 1. Where things stand

Established by inspection of `origin/master` (7512973, v0.8.1).

| Fact | Evidence |
|---|---|
| The only in-repo simulator is a library, `pic14-sim` (`crates/sim`, `publish = false`). It has no CLI. | `crates/sim/Cargo.toml`, `crates/sim/src/lib.rs` |
| The sim covers all four cores: `Pic14`, `Pic14e`, `PicBaseline`, `Pic18`, each with `new`/`with_device`, `step`, `run(max_steps)`, `run_until(target, max_steps) -> StopReason`, `cycles()`, `halted()`, `pc()`, `w()`, `ram()`/`ram_mut()`, `read_mem`/`write_mem`. | `crates/sim/src/lib.rs` (types at lines 361, 1077, 1963, 2561) |
| Stop conditions today are narrow: `StopReason::{Reached, Halted, Capped}`; halt means SLEEP or running off the end of the program. | `crates/sim/src/lib.rs:961` |
| HEX loading already exists per core: `parse_hex` (PIC14 and baseline), `parse_hex_pic14e`, `parse_hex_pic18`. The parsers panic on malformed input, which is the current error surface. | `crates/sim/src/lib.rs:11,2453,2502` |
| Device selection already exists: `device::resolve(name)` accepts `p16f877a`, `16F877A`, and `PIC16F877A` spellings, and `Device.core` gives `Core::{Pic14, Pic14e, PicBaseline, Pic18}`. `device::ALL` lists every part. | `crates/device/src/lib.rs:15,142` |
| The debugger adapter already solved core dispatch from a device name: `core_of` and `load_program` in the gdbserver crate map a device to a core and a core to a HEX parser. It is the nearest sibling to mirror. | `crates/gdbserver/src/lib.rs:131,148` |
| The gdbserver is not a newcomer path: it needs a sidecar ELF argument, a TCP port, and a gdb client. | `crates/gdbserver/src/main.rs`, `USAGE` at `crates/gdbserver/src/lib.rs:49` |
| CLI precedent is hand-rolled argument parsing, no clap anywhere. Unknown flags get a did-you-mean hint. | `crates/gdbserver/src/lib.rs:88`, `crates/driver/src/cli.rs` |
| No peripheral output model exists in the sim: no UART, TXREG, or PORT model. Tests observe results by reading RAM (`ram_mut`, `write_mem`) and by `fire_interrupt`/`request_interrupt` hooks. There is one peripheral exception, the PIC18 USB SIE (`crates/sim/src/sie`, driven by `Pic18::usb_*`), which is out of scope here. | `crates/sim/tests/*.rs`, `crates/sim/src/lib.rs` |
| The release bundle already ships two binaries, `epic-cc` and `epic-cc-gdbserver`, at the zip root, plus `devices.json`, `include/`, bundled `clang/`, and the LLVM licence. | `docs/internals/design/30-distribution-design.md`, bundle layout section; `Dockerfile` release stage |
| Linux builds the bundle in the Dockerfile `release` stage (`cargo build --release -p driver -p epic-cc-gdbserver`, then copies). Windows builds natively in `release.yml`. Both already ship the gdbserver, so the earlier note that Windows ships the driver only is stale. | `Dockerfile:247-252`, `.github/workflows/release.yml:73-96` |
| A bundle gate exists: `scripts/check_release_bundle.py` fails unless each zip carries both binaries. | `scripts/check_release_bundle.py` (`BINARIES`) |
| Version truth is one stamp: `EPIC_CC_STAMP`, emitted by a per-crate `build.rs`, printed by `--version`. | `crates/driver/build.rs`, `crates/gdbserver/build.rs`, `crates/gdbserver/src/main.rs:18` |
| Licence: the repo is MIT and the sim depends only on the `device` crate (std only, no third-party crates), so a runner over it ships under MIT with no new licence file. The GPL boundary (gpasm, gpsim) is untouched: neither is linked or invoked. | `LICENSE`, `crates/sim/Cargo.toml` |
| The current tutorial path, `epic-hal make sim-epiccc`, lives in epic-hal, needs both checkouts and the dev image, and is exactly what this ticket removes for newcomers. It stays available for HAL work; nothing here replaces it for its own users. | epic-cc#1293 |

Two consequences fall out of the table. First, the runner is a thin
front end over finished machinery: parsing, device resolution, core
dispatch, stepping, and cycle counting all exist and are tested
through the gdbserver and the sim test suite. Second, the one thing
that does not exist is an output model (what the user sees a program
do), so that is the real design choice, taken in D-4.

## 2. Decisions

### D-1: A standalone `epic-cc-sim` binary, in its own crate

Ship a third binary, `epic-cc-sim` (`epic-cc-sim.exe` on Windows),
from a new crate `crates/sim-runner` (package `epic-cc-sim`, one
`[[bin]]`, a `build.rs` mirroring the gdbserver one for
`EPIC_CC_STAMP`). It depends only on `pic14-sim` and `device`.

Rationale: this is the shape the repo already uses for its second
binary. `crates/gdbserver` is a separate crate wrapping the same sim
library behind a different front end, and the release machinery
already knows how to build, copy, smoke-test, and gate a second
binary. A third follows the same grooves. Keeping the runner out of
`crates/sim` also keeps `pic14-sim` a pure embeddable library (the
fuzz crate and the test suites link it without CLI or IO surface).

**Rejected, `epic-cc sim` subcommand (brief option 2).** One binary
is easier to discover, but the driver then links the simulator,
which blurs the compiler's scope, grows its test surface, and makes
every compiler invocation carry sim startup cost and failure modes.
If discoverability later matters, a subcommand that forwards to the
runner is a small follow-up; the reverse move (splitting a grown
subcommand back out) is not.

**Rejected, shipping the epic-hal sim runner (brief option 3).** It
drags epic-hal harness code and its peripheral models into the
epic-cc release, and the release pipeline would need a cross-repo
artifact at a pinned version at tag time. That coupling is the
problem the ticket exists to remove. See also section 6.

**Rejected, a `[[bin]]` inside `crates/sim`.** Fewer files, but the
sim crate then owns argument parsing, stdout formatting, and exit
codes, and its library consumers inherit a binary target they never
asked for. The gdbserver precedent (separate crate per front end)
wins.

### D-2: No new dependencies, std only

The runner uses only `pic14-sim`, `device`, and std. No clap (the
repo hand-rolls argument parsing in both existing CLIs, with the
same did-you-mean hint shape), no serde, no terminal crates. This
keeps the binary small, the build fast, and the licence surface
unchanged (MIT, nothing new to ship or attribute).

### D-3: All four cores from day one, one dispatch

The runner resolves `--device` through `device::resolve` and
dispatches on `Device.core`, mirroring `core_of`/`load_program` in
`crates/gdbserver/src/lib.rs`: `parse_hex` for PIC14 and baseline,
`parse_hex_pic14e` for PIC14E, `parse_hex_pic18` for PIC18. The
runner holds the four sim types behind one internal enum (the same
pattern as the gdbserver's four target structs) so the run loop,
stimulus, and output code are written once.

Rationale: the dispatch costs almost nothing (the parsers and cores
already exist and the gdbserver already wrote this exact match),
while shipping PIC14-only would force a second design round the
first time a tutorial targets a PIC14E or PIC18 part. Baseline needs
a `&'static Device` for `PicBaseline::with_device`; the resolved
device supplies it, with `p12f509` as the fallback when no device is
named on a baseline part (the gdbserver does the same).

The default device is `p16f877a`, the part the tutorials and the
repo's own examples target today, so the zero-flag invocation
`epic-cc-sim blink.hex` does the expected thing.

### D-4: Output is an SFR write trace plus a final state line

This is the load-bearing choice. The sim has no peripherals, so
"the program printed hello" cannot be observed today. Three models
were considered:

- **A. SFR write trace (chosen).** The user names registers with
  `--trace-sfr PORTB,TXREG`; every write to a traced register prints
  one line (format in section 3). No firmware cooperation is needed,
  so LED blink, GPIO, and bit-banged serial tutorials all work with
  the HEX they already produce. A UART tutorial traces TXREG and
  reads the byte values from the trace; reconstructing a text
  stream from those bytes is a reading task for the tutorial, not
  new sim machinery.
- **B. Putchar trap (deferred).** A reserved RAM address acts as a
  stdout byte sink, giving precise text. It needs a firmware shim
  (a three-line `putch`), a magic address convention that must never
  collide with a real allocation, and documentation in every
  tutorial. It is a good follow-up if tutorials ask for text output;
  it is not needed to unblock them, and adding it now would spend
  the design budget on the one feature with a new user-visible
  convention.
- **C. A real UART/peripheral model in the sim (rejected for v1).**
  Modelling USART timing, baud rates, and pin behaviour is a sim
  project of its own, per core, with datasheet-grade correctness
  questions. It blocks the tutorial on the largest possible scope.

Implementation note for A: the sim has no write callback, and v1
adds none. The runner steps the program and, after each step,
compares the traced addresses against their previous values in
`ram()`. One step is one instruction, so no write is missed, and a
write of the same value is still caught by comparing before and
after the step rather than by watching for change across a run.
This costs one slice comparison per traced address per step, which
is irrelevant at tutorial scale, and it keeps the change inside the
new crate. If profiling ever objects, a watch hook in `pic14-sim`
is a separable follow-up that does not change the CLI.

Register names resolve through `Device.sfrs` (name and aliases,
`crates/device/src/lib.rs`), so `PORTB` and its legacy spellings
work on devices whose pack data carries SFR names. Where a device
has no SFR table, or a name is unknown, a numeric address
(`--trace-sfr 0x06`) always works, and an unknown name is a usage
error listing the problem rather than a silent no-trace run.

### D-5: Stimulus is pokes, timed by cycle, from flags or a file

Tutorials need to drive inputs (a button on a pin, a sensor byte in
a register), which today tests do with `ram_mut`/`write_mem` before
and between steps. The runner exposes exactly that, in two forms:

- `--poke ADDR=BYTE` (repeatable): an initial write, applied before
  the first step, through the core's `write_mem` so bank resolution
  matches the machine's own operands.
- `--stimulus FILE`: a plain text file, one action per line, applied
  when the sim's cycle count reaches the stated cycle:

  ```text
  # comment
  at 1000 poke 0x20 0x01
  at 2500 interrupt
  ```

  `poke` writes one byte (same addressing as `--poke`); `interrupt`
  calls `request_interrupt` (the gated hook that respects INTCON,
  not the unconditional `fire_interrupt` test hook, so stimulus
  behaves the way hardware would). Blank lines and `#` comments are
  ignored. Anything else is a parse error naming the file and line.

The run loop therefore steps (or runs in slices bounded by the next
stimulus cycle, the next breakpoint, or the cycle cap) instead of
calling `run` once. This is the same loop the gdbserver's resume
path already implements per core; the runner writes it once over
its dispatch enum.

**Rejected, a general scripting or interactive mode.** A REPL,
breakpoint commands, or a stimulus language with expressions would
make the runner a second debugger, competing with
`epic-cc-gdbserver` (design 34). The runner is deliberately a batch
tool: fixed inputs, fixed output, an exit code. Interactive work
already has a home.

### D-6: The bundle gains one binary, nothing else moves

Packaging changes, all mechanical:

- Dockerfile `release` stage: add `-p epic-cc-sim` to the cargo
  build and one `cp` line next to the gdbserver copy.
- `release.yml` Windows job: same `-p` addition and one `Copy-Item`.
- `scripts/check_release_bundle.py`: add `epic-cc-sim` to
  `BINARIES`, so a zip missing the runner fails the release.
- Smoke tests: each platform already compiles `add.c` with the
  shipped `epic-cc` and byte-diffs the HEX. Extend the smoke to run
  that HEX under the shipped `epic-cc-sim` with a cycle cap and
  assert the halted line (section 3) on stdout and exit code 0.
  The Linux smoke runs inside the Dockerfile `release` stage build;
  the Windows smoke next to the existing gdbserver listen test.
- No zip layout change: the binary sits at the bundle root beside
  `epic-cc` and `epic-cc-gdbserver`. No new directory, no PATH or
  discovery machinery (the runner finds nothing next to itself; it
  reads only the HEX and stimulus paths it is given).

Size impact: the runner is a std-only Rust binary over two in-repo
crates. Expect roughly 1 to 2 MB uncompressed per platform (the
same order as `epic-cc-gdbserver`, which the implementation ticket
must measure and record in its PR), and well under 1 MB of zip
growth against a bundle dominated by the bundled clang tree. That
is noise for the download and buys the whole tutorial path. The
implementation ticket records the real numbers (`ls -l` on both
binaries, zip listing before and after) rather than trusting this
estimate, and flags it in review if the runner lands over 3 MB.

Licence impact: none. MIT in, MIT out; no new third-party licence
file in the zip. The GPL boundary is untouched (no gpasm or gpsim
linking or invocation, in the runner or its tests).

## 3. CLI contract

New public interface. Everything in this section is the contract
tutorials and epic-docs will depend on.

```text
epic-cc-sim <hex> [--device <name>] [--cycles <n>]
            [--break <addr>]... [--poke <addr>=<byte>]...
            [--stimulus <file>] [--trace-sfr <name|addr>[,...]]
            [--dump regs|ram|all]
epic-cc-sim --help
epic-cc-sim --version
```

| Input | Meaning | Default |
|---|---|---|
| `<hex>` | Intel HEX file to run (positional, required). Parsed by the core the device selects. | none |
| `--device <name>` | Any spelling `device::resolve` accepts (`p16f877a`, `16F877A`, `PIC16F877A`). Selects core, RAM map, and SFR names. | `p16f877a` |
| `--cycles <n>` | Stop when the sim cycle count reaches `n`. Accepts plain decimal and `0x` hex. | `1000000` |
| `--break <addr>` | Stop before executing the instruction at this word address (byte address on PIC18, matching `Pic18::pc`). Repeatable. | none |
| `--poke <addr>=<byte>` | Initial data-memory write before step one. Repeatable. | none |
| `--stimulus <file>` | Timed pokes and interrupts (D-5). | none |
| `--trace-sfr <list>` | Comma-separated register names or addresses to trace (D-4). | none |
| `--dump <what>` | Final dump level: `regs` (registers only), `ram` (registers plus non-zero RAM), `all` (registers plus full RAM and EEPROM). | `regs` |

Behaviour:

- The program runs from reset until it halts, a breakpoint is hit,
  the cycle cap is reached, or stimulus is exhausted and the cap
  follows. There is no interactive prompt and no paging.
- Output goes to stdout and is machine-stable: tutorials and tests
  may diff it byte for byte. Diagnostics go to stderr. Nothing else
  is printed; in particular there is no banner and no progress
  output, however long the run.
- Trace lines (only for `--trace-sfr` registers, in execution
  order), one per write:

  ```text
  cycle=1234 pc=0x00ff write PORTB=0x02
  ```

  The name shown is the spelling the user gave, or the canonical
  SFR name when an address was given and the device knows it.
- Exactly one final line, always last on stdout:

  ```text
  halted after 1234 cycles pc=0x0100 w=0x02 status=0x18
  ```

  The first word is the stop reason: `halted`, `breakpoint`, or
  `capped`. With `--dump ram|all`, the dump follows the final line
  (registers as `name=0xNN` lines, RAM as `ram 0x20: 01 02 ...` rows
  of 16 bytes, EEPROM likewise under `eeprom`), so a diffing
  tutorial can match the final line as a prefix.
- Exit codes, also part of the contract:

  | Code | Meaning |
  |---|---|
  | 0 | The program halted (`halted` final line). |
  | 1 | Runtime error: HEX unreadable or malformed, stimulus file unreadable or malformed. The diagnostic is on stderr. |
  | 2 | Usage error: unknown flag, bad value, unknown device, unknown SFR name, missing `<hex>`. Usage text follows the diagnostic on stderr. This matches the gdbserver's usage exit. |
  | 3 | The cycle cap was reached (`capped` final line). Distinct from 0 so a tutorial script can tell "ran to completion" from "stopped early". |
  | 4 | A breakpoint was hit (`breakpoint` final line). |

  Note this deliberately differs from the exit sketch in the
  ticket's early comments (which put the cap at 2): 2 is kept for
  usage so both shipped CLIs agree on what a bad invocation costs,
  and the cap and breakpoint get their own codes because scripts
  branch on them.
- `--help` prints usage to stdout and exits 0; `--version` prints
  `epic-cc-sim <EPIC_CC_STAMP>` and exits 0, the gdbserver shape.
- Malformed HEX: the sim parsers panic today. The runner catches
  that at its boundary (read the file itself, validate record shape
  before handing text to the parser) and reports it as exit 1 with
  the offending line, rather than leaking a panic and backtrace
  hint at a newcomer. This follows the repo's stated direction that
  a panic reaching a user is a defect (design 46, section 1), scoped
  to the new binary only; hardening the parsers themselves is a
  separable sim follow-up.

## 4. How it relates to the existing code

- **`crates/sim` (`pic14-sim`) is the engine, unchanged in v1.** The
  runner consumes the public surface listed in section 1. No new
  sim API is required: stimulus uses `write_mem` and
  `request_interrupt`, tracing uses `ram()` reads around `step()`,
  breakpoints use the loop condition (or `run_until` where a slice
  has a single target), and the final line uses `cycles()`, `pc()`,
  `w()`, and the core's status accessor. If a core lacks a uniform
  accessor the runner reads the status register from RAM at the
  core's known address, as the sim's own tests do.
- **`crates/gdbserver` is the sibling to mirror, not a dependency.**
  The runner copies its shapes (hand-rolled `parse_args` returning
  `Result<_, String>`, a public `USAGE` constant, `core_of`-style
  dispatch, per-crate `build.rs` stamp) but does not link it: the
  gdbserver's gdbstub dependency and TCP serve loop are exactly
  what the runner must not carry. Where the dispatch code is
  genuinely identical, copying 30 lines beats a shared crate; a
  shared front-end crate is justified only when a third consumer
  appears.
- **`crates/device` is the registry, unchanged.** Resolution, core
  selection, RAM maps, and SFR names all come from it.
- **`crates/driver` is untouched.** The compiler does not link the
  sim (D-1), gains no flag, and its bundle discovery chain is
  irrelevant to the runner, which locates no sibling files.
- **Tests today** drive the sim from `crates/sim/tests/` (per-core
  suites, `hex.rs` parser tests) and end-to-end through the driver
  and gdbserver suites. The runner adds its own test home (section
  7) and reuses fixture HEX files compiled from the driver's
  existing fixtures where practical, so the runner's tests exercise
  real compiler output rather than hand-assembled words.

## 5. What the tutorial looks like after this ships

```text
$ unzip epic-cc-0.9.0-x86_64-linux.zip
$ cd epic-cc-0.9.0-x86_64-linux
$ ./epic-cc blink.c -o blink.hex
$ ./epic-cc-sim blink.hex --trace-sfr PORTB --cycles 50000
cycle=812 pc=0x003a write PORTB=0x01
cycle=2504 pc=0x003a write PORTB=0x00
capped after 50000 cycles pc=0x0012 w=0x00 status=0x18
```

(exit code 3: a blink loop never halts, and the tutorial says so.)
No checkout, no image, no environment variable. This walkthrough is
the acceptance test the epic-docs getting-started page will encode.

## 6. What stays out

- **No epic-hal dependency of any kind.** The runner neither links,
  shells out to, nor reads epic-hal code, headers, or harness files.
  epic-hal's `make sim-epiccc` keeps working for HAL development;
  it is simply no longer on the newcomer's path, and no HAL
  peripheral model is imported to fill the output gap (D-4 chose the
  trace instead).
- **No debugger features.** No REPL, no stepping commands, no
  watchpoints beyond the trace, no sidecar or symbol reading.
  `epic-cc-gdbserver` (design 34) owns interactive debugging.
- **No peripheral modelling.** No USART timing, ADC, timers beyond
  what the sim already counts, or pin electronics. Traced SFR
  writes are observed values, not a device model.
- **No putchar trap in v1** (D-4, option B). Deferred, with its
  trigger: a tutorial that needs text output rather than register
  values asks for it, and it lands as its own small design note
  because the magic address is a new convention.
- **No C source input.** The runner takes HEX only. Compiling is
  `epic-cc`'s job; a combined compile-and-run flag is sugar that
  can be added later without changing this contract.
- **No JSON output in v1.** The text format is specified as
  diff-stable precisely so scripts can consume it. A `--format
  json` flag is a compatible addition if a consumer appears; it is
  not designed here.
- **No new sim watch or callback API in v1** (D-4 note), and no
  change to `pic14-sim` behaviour, the parsers' panic surface
  outside the runner's own boundary, or any existing binary's CLI.
- **No PlatformIO change in this repo's follow-ups.** The PIO
  toolchain package consumes the same zip (design 30, D-8); picking
  up the new binary there is an epic-platformio follow-up once the
  zip carries it.

## 7. Implementation breakdown

Sized for follow-up tickets, in dependency order. Each is one PR;
T1 to T3 are the critical path, T4 can overlap T3, T5 closes the
ticket, T6 is conditional.

| Ticket | Scope | Size | Depends on |
|---|---|---|---|
| T1: `crates/sim-runner` skeleton | New crate, `build.rs` stamp, hand-rolled `parse_args` with `USAGE`, `--help`/`--version`, device resolution, per-core HEX load, run to halt or cap, final line, exit codes 0/1/2/3. Unit tests for argument parsing; an integration test running a fixture HEX to `halted`. | M (about 300 lines plus tests) | none |
| T2: Breakpoints, pokes, and stimulus | `--break`, `--poke`, `--stimulus` file parser, the sliced run loop, `request_interrupt` stimulus, exit code 4. Tests: timed poke observed by the program, interrupt stimulus taken, malformed stimulus line exits 1 naming file and line. | M | T1 |
| T3: SFR trace and dumps | `--trace-sfr` name/address resolution over `Device.sfrs`, step-and-compare tracing, `--dump regs|ram|all`. Tests: PORTB trace lines in order on a blink fixture, unknown SFR name exits 2, dump formats pinned by golden files. | M | T1 |
| T4: Bundle packaging | Dockerfile `release` stage and `release.yml` Windows job build and copy the binary; `check_release_bundle.py` requires it; per-platform smoke runs the shipped runner on the smoke HEX and asserts the final line and exit 0. Record measured binary and zip sizes in the PR. | S | T1 (T2/T3 not required: smoke needs only halt) |
| T5: Tutorial readiness | README quickstart gains the compile-then-simulate flow (user-visible surface, so the README rule applies); a short user doc under `docs/user/` for the runner's flags, output, and exit codes, generated from the same `USAGE` where the docs generator allows. Verify the section 5 walkthrough from a clean directory with only the zip. | S | T4 |
| T6 (conditional): Putchar trap | Only if a tutorial needs text output. Small design note for the magic address, sim or runner support, tutorial shim. Not filed until the need is concrete. | S to M | T3, a requesting tutorial |

Total for T1 to T5: roughly 600 to 900 lines including tests,
spread over three M and two S tickets, plus the packaging lines.
No ticket touches `crates/driver`, `crates/sim` internals, or any
existing CLI. The review gate and takeoff ritual apply per PR as
usual, and each PR that changes user-visible surface carries its
README note.

## 8. Risks and open questions for the approver

- **Trace cost on long runs.** Step-and-compare is one instruction
  at a time; a tutorial running tens of millions of cycles with no
  trace flags should use the bulk `run` path when no breakpoint or
  stimulus is pending. The run loop in T2 must slice on the next
  event (stimulus cycle, breakpoint, cap) and bulk-run between
  events, or `--cycles 100000000` becomes a slow way to learn
  nothing. This is a design requirement on T1/T2, recorded here so
  review can check it.
- **Devices without SFR tables** get numeric tracing only. If the
  beta board set (design 46, D-2) is the tutorial target list, T3
  should confirm those parts carry `sfrs` data and file a device
  data follow-up for any that do not.
- **PIC18 address width.** `Pic18::pc` is `u32` and its breakpoints
  are byte addresses while the other cores take word addresses.
  The contract states this rather than hiding it; tutorial authors
  for PIC18 must be told, which is T5's job.
- **Decision still owed by the human:** approve D-1 (standalone
  binary) with D-4 option A (SFR trace), or redirect to the
  subcommand (option 2 in the ticket comments) or the putchar trap
  in v1. This document recommends D-1 plus A, for the reasons in
  section 2. Implementation tickets are filed only after that
  approval, per the repository's design-before-code rule.
