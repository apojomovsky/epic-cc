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
| The sim covers all four cores, but their surfaces differ and the runner must not paper over it: `Pic14` has `new(prog)` (= `with_device(&PIC16F877A, prog)`) and `with_device(&'static Device, prog)`; `Pic14e` has both, but its `with_device(&Device, prog)` checks only the `Core::Pic14e` tag, not per-device fields; `PicBaseline` has only `with_device(&Device, prog)` (no `new`); `Pic18` has only `new(prog)` (no `with_device`, device ignored entirely). All four have `step`, `run(max_steps)`, `run_until(target, max_steps) -> StopReason`, `cycles()`, `halted()`, `pc()` (`u16` except `Pic18: u32`), `w()`, `status()`, `ram()`/`ram_mut()` (256 B baseline, 512 B Pic14, 4096 B Pic14e/Pic18), and `eeprom()` except `PicBaseline`, which has none. `read_mem`/`write_mem` exist on all four but with incompatible address semantics (see D-5). | `crates/sim/src/lib.rs` (types at lines 361, 1077, 1963, 2561) |
| Stop conditions today are narrow: `StopReason::{Reached, Halted, Capped}`; halt means SLEEP or running off the end of the program. | `crates/sim/src/lib.rs:961` |
| HEX loading already exists per core: `parse_hex` (PIC14 and baseline), `parse_hex_pic14e`, `parse_hex_pic18`. The parsers panic on malformed input, which is the current error surface. | `crates/sim/src/lib.rs:11,2453,2502` |
| Device selection already exists: `device::resolve(name)` accepts `p16f877a`, `16F877A`, and `PIC16F877A` spellings, and `Device.core` gives `Core::{Pic14, Pic14e, PicBaseline, Pic18}`. `device::ALL` lists every part. | `crates/device/src/lib.rs:15,142` |
| The debugger adapter already solved part of core dispatch: `core_of` maps a device name to a core and `load_program` maps a core to a HEX parser in the gdbserver crate. Its construction dispatch is narrower than it looks: it calls `Pic14::new`/`Pic14e::new`/`Pic18::new` (default devices, resolved device ignored) and uses the device only for baseline (`PicBaseline::with_device`, defaulting to `p12f509` when none was named). The runner mirrors the parser mapping, not that construction shortcut (D-3). | `crates/gdbserver/src/lib.rs:131,148` |
| The gdbserver is not a newcomer path: it needs a sidecar ELF argument, a TCP port, and a gdb client. | `crates/gdbserver/src/main.rs`, `USAGE` at `crates/gdbserver/src/lib.rs:49` |
| CLI precedent is hand-rolled argument parsing, no clap anywhere. Unknown flags get a did-you-mean hint. | `crates/gdbserver/src/lib.rs:88`, `crates/driver/src/cli.rs` |
| Peripheral modelling is partial, not absent: Timer0 exists on all cores, Timer2 on Pic18, plus ADC stubs and data-EEPROM models on the cores that have EEPROM; `request_interrupt`/`fire_interrupt` exist on `Pic14`, `Pic14e`, and `Pic18` only — `PicBaseline` has neither (12F509-class parts have no interrupts). There is still no UART/TXREG or pin/PORT output model: SFR cells such as TXREG are plain RAM the trace can observe. The PIC18 USB SIE lives in `crates/sim/src/sie.rs` (driven by `Pic18::usb_*`) and is out of scope here. | `crates/sim/tests/*.rs`, `crates/sim/src/lib.rs`, `crates/sim/src/sie.rs` |
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
dispatches on `Device.core`. It mirrors only the HEX-parser mapping
of `load_program` in `crates/gdbserver/src/lib.rs` (`parse_hex` for
PIC14 and baseline, `parse_hex_pic14e` for PIC14E,
`parse_hex_pic18` for PIC18); construction is per core, because the
constructors genuinely differ and `--device` must not promise what
a core ignores:

| Core | Construction | What `--device` actually selects |
|---|---|---|
| `Pic14` | `Pic14::with_device(resolved, prog)` (`&'static Device`, which `device::resolve` returns) | Core, RAM bank map, SFR names |
| `Pic14e` | `Pic14e::with_device(resolved, prog)` | Core and SFR names; the sim checks only the core tag, so bank/linear maps are per-core constants, not per-device |
| `PicBaseline` | `PicBaseline::with_device(resolved, prog)` (no `new` exists) | Core, file map, SFR names |
| `Pic18` | `Pic18::new(prog)` (no `with_device` exists) | Core, HEX parser, and SFR names only; the RAM map is the sim's fixed 4096-byte map regardless of device |

The runner holds the four sim types behind one internal enum (the
same pattern as the gdbserver's four target structs) so the run
loop, stimulus, and output code are written once. The enum is not
free: `pc()` returns `u16`/`u32`, `ram()` returns 256/512/4096-byte
arrays, and EEPROM and interrupt support differ per core, so each
enum method is a four-way match (priced in section 7).

Rationale: the parsers and cores already exist, while shipping
PIC14-only would force a second design round the first time a
tutorial targets a PIC14E or PIC18 part.

The default device is `p16f877a`, the part the tutorials and the
repo's own examples target today, so the zero-flag invocation
`epic-cc-sim blink.hex` does the expected thing. A baseline part is
therefore only ever selected by an explicit `--device`, and the
resolved device always supplies the `&'static Device`
`PicBaseline::with_device` needs; there is no fallback device.

### D-4: Output is an SFR write trace plus a final state line

This is the load-bearing choice. The sim has no output
peripheral, so "the program printed hello" cannot be observed
today. Three models were considered:

- **A. SFR write trace (chosen).** The user names registers with
  `--trace-sfr PORTB,TXREG`; every change to a traced register
  prints one line (format in section 3; change-only in v1, see the
  implementation note below). No firmware cooperation is needed,
  so LED blink, GPIO, and bit-banged serial tutorials all work with
  the HEX they already produce. A UART tutorial can trace TXREG
  and read changing byte values from the trace, but v1 does not
  promise a reconstructable text stream (repeated bytes are lost);
  that needs the deferred watch hook (T6b) or the putchar trap.
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
adds none. The runner steps the program one instruction at a time
(D-5) and, after each step, compares each traced register's value
against its value before the step. **The v1 trace is therefore
change-only: a write of the same value produces no line, because a
value comparison cannot detect it.** That is the contract, stated
plainly rather than implied: LED blink, GPIO, and bit-banged
tutorials (which toggle values) are unaffected, but a TXREG trace
loses repeated bytes (`"ll"` in `"hello"` prints once), so v1 does
not promise a reconstructable UART byte stream. Recovering
same-value writes requires a sim watch hook; that is deferred
priced work, not a v1 silent gap: a `pic14-sim` hook fired from
each core's `write_f` (four call sites, plus a runner callback
registration) is estimated at S–M (roughly 100–200 lines in
`crates/sim` plus tests) as its own ticket (T6b in section 7),
filed only when a tutorial needs the byte stream, and it does not
change the CLI when it lands.

What a traced register's "value" is, per register kind:

- **RAM-backed registers (the normal case):** the `width` bytes at
  the register's physical address in `ram()` (see D-5 for the
  address space), compared as a byte string. `Sfr.width` is 1 for
  plain SFRs and 2–3 for joined registers (`TMR1` over
  `TMR1L:TMR1H`, `crates/device/src/lib.rs`); one line is emitted
  per step in which any of those bytes changed, showing the bytes
  assembled little-endian (low byte at the lowest address) as
  `0x` plus exactly `2 × width` lowercase hex digits. A register
  traced by numeric address always has width 1.
- **Pseudo-registers not backed by the compared cell** are traced
  through their accessor instead of `ram()`: `PCL` (physical
  0x02 on Pic14/Pic14e/PicBaseline; reads as `pc() & 0xFF`, and
  the `ram[0x02]` cell is not the live value) snapshots
  `pc() & 0xFF`, and Pic14e `WREG` (file 0x09, backed by the `w`
  field, not `ram[0x09]`) snapshots `w()`. Pic18 has the same two
  pseudo-registers at different addresses: `WREG` (0xFE8, backed
  by `w`, snapshot `w()`) and `PCL` (0xFF9, snapshot
  `pc() & 0xFF`; the core's execution-time read adds the prefetch
  offset, but the traced value is defined as the post-step PC low
  byte so all cores agree). `INDF` (0x00 on Pic14/Pic14e/PicBaseline) is
  not traceable: its value depends on FSR at access time, so
  naming or addressing it in `--trace-sfr` is a usage error
  (exit 2) that says to trace the target register instead; the
  Pic18 `INDF0`/`INDF1`/`INDF2` family is rejected the same way.

Register names resolve through `Device.sfrs` (name and aliases,
`crates/device/src/lib.rs`), so `PORTB` and its legacy spellings
work on devices whose pack data carries SFR names. Where a device
has no SFR table, or a name is unknown, a numeric address
(`--trace-sfr 0x06`) always works, and an unknown name is a usage
error listing the problem rather than a silent no-trace run.
Name/alias and display rules are in section 3.

### D-5: Stimulus is pokes, timed by cycle, from flags or a file

Tutorials need to drive inputs (a button on a pin, a sensor byte in
a register), which today tests do with `ram_mut`/`write_mem` before
and between steps. The runner exposes that in two forms, over one
explicitly defined address space:

**Address semantics (decided here; there is no single `ADDR` in
the sim today).** Every address the CLI takes or prints for data
memory — `--poke`, stimulus `poke`, `--trace-sfr` numeric
addresses, and dump offsets — is a **flat physical index into the
core's `ram()` array**, the same space the trace compares and the
dump prints. Per core that space is 0x000–0x0FF (PicBaseline,
256 B), 0x000–0x1FF (Pic14, 512 B), or 0x000–0xFFF (Pic14e and
Pic18, 4096 B); `Device.sfrs` addresses are already physical in
this sense (TRISB is 0x86 on the 877A, not 0x06-plus-a-bank). The
runner deliberately does **not** route pokes through
`write_mem`: `Pic14::write_mem(addr: u8)` asserts `addr <= 0x7F`
and resolves through whichever bank is live at that cycle (so
`--poke 0x86` would panic and a timed poke would land in a
bank-dependent cell), while the other three cores' `write_mem`
is already flat physical. One CLI number must mean one cell, at
any cycle, on every core, so the runner writes the validated
physical cell directly through `ram_mut()`. Consequence, stated
for tutorial authors: a poke is a raw cell write — it does not
fire the EEPROM/ADC/Timer0 side effects that an instruction
store to the same SFR would, and it does not update the `w`
field behind a pseudo-register such as Pic14e `WREG`; poking a
pseudo-register address writes the dead `ram()` cell, so the
runner rejects pokes to the D-4 pseudo-register and `INDF`
addresses instead of silently writing a cell nothing reads.

**Validation, so no panic reaches the user.** All addresses and
bytes are validated before any sim call, and the runner only ever
indexes `ram()`/`ram_mut()` with a validated address, so the
sim's internal `assert!`s on `write_mem`/`read_mem` are never
reached (the runner does not call them at all):

| Check | `--poke` / `--trace-sfr` (flags) | stimulus `poke` |
|---|---|---|
| Address parses as a number, byte as 0–0xFF | exit 2 | exit 1, naming file and line |
| Address inside the core's physical range above | exit 2 | exit 1, naming file and line |
| Traced range `addr + width` inside the range | exit 2 | n/a |
| Pseudo-register / `INDF` address | exit 2 | exit 1, naming file and line |

The whole stimulus file is parsed and validated up front (syntax,
ranges, and the baseline-interrupt rule below) before the first
step, so a bad line fails fast instead of mid-run.

The two forms:

- `--poke ADDR=BYTE` (repeatable): an initial physical write,
  applied before the first step.
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
  behaves the way hardware would) **on Pic14, Pic14e, and Pic18
  only. PicBaseline has no interrupt hook and its parts have no
  interrupts, so an `interrupt` line for a baseline device is a
  validation error: exit 2** (the invocation can never do what it
  asks on that device, the same class as an unknown SFR name),
  naming the file and line, detected up front. v1 adds no sim API
  to change this. Blank lines and `#` comments are ignored.
  Anything else is a parse error naming the file and line.

**Run loop (decided here).** The loop always steps one instruction
at a time; it never calls `run`/`run_until`. Those primitives
count steps (instructions cost 1–2 cycles), take a single target,
and cannot express "stop at the earliest of several breakpoints,
the exact cycle cap, or the next stimulus cycle", so the sliced
bulk-run loop an earlier draft sketched has no primitive to stand
on and is removed. Per step, in order: apply stimulus actions
whose cycle has been reached, check breakpoints against the
pre-step `pc()`, snapshot traced registers, `step()`, emit trace
lines for changed registers, then check `halted()` and the cycle
cap (`cycles() >= cap`; a 2-cycle instruction may overshoot the
cap by one, which the capped final line reports truthfully).
Adding a cycle/target-set API to `pic14-sim` to recover bulk
running was considered and rejected for v1: it is new sim API
with its own design questions, and per-step stepping is the
honest price of exact stimulus/breakpoint/cap semantics. The
performance consequence is accepted and documented in section 8;
a bulk fast path is separable follow-up work that must not change
observable output.

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
| `--device <name>` | Any spelling `device::resolve` accepts (`p16f877a`, `16F877A`, `PIC16F877A`). Selects core, HEX parser, and SFR names; it selects the RAM map only where the core takes a device (D-3 table — Pic18 ignores it). | `p16f877a` |
| `--cycles <n>` | Stop when the sim cycle count reaches `n`. Accepts plain decimal and `0x` hex. | `1000000` |
| `--break <addr>` | Stop before executing the instruction at this word address (byte address on PIC18, matching `Pic18::pc`). Repeatable. | none |
| `--poke <addr>=<byte>` | Initial write before step one; `<addr>` is a flat physical `ram()` index (D-5). Repeatable. | none |
| `--stimulus <file>` | Timed pokes and interrupts (D-5). `interrupt` is rejected with exit 2 on PicBaseline devices. | none |
| `--trace-sfr <list>` | Comma-separated register names or physical addresses to trace (D-4). Change-only in v1. | none |
| `--dump <what>` | Dump printed before the final line: `regs` (registers only), `ram` (registers plus non-zero RAM), `all` (registers plus full RAM and, where the core has one, EEPROM). | none (no dump) |

Behaviour:

- The program runs from reset until it halts, a breakpoint is hit,
  the cycle cap is reached, or stimulus is exhausted and the cap
  follows. There is no interactive prompt and no paging.
- Output goes to stdout and is machine-stable: tutorials and tests
  may diff it byte for byte. Diagnostics go to stderr. Nothing else
  is printed; in particular there is no banner and no progress
  output, however long the run.
- Trace lines (only for `--trace-sfr` registers, in execution
  order), one per step in which the register's value changed
  (D-4; same-value writes emit no line in v1):

  ```text
  cycle=1234 pc=0x00ff write PORTB=0x02
  ```

  `cycle=` is the sim cycle count after the step, in decimal.
  `pc=` is the address of the instruction that made the change —
  the `pc()` captured **before** the step — formatted `0x` plus
  lowercase hex, zero-padded to 4 digits on Pic14/Pic14e/
  PicBaseline and 6 digits on Pic18. The value is `0x` plus
  exactly `2 × width` lowercase hex digits (D-4). The name shown
  is always the canonical `Sfr.name`: a name or alias the user
  gave resolves to its canonical entry, an address resolves to
  the **first** entry in `Device.sfrs` order whose `addr` equals
  it (deterministic when several SFRs alias one address), and an
  address with no entry displays as `0x` plus the padded address
  (2 digits baseline, 3 digits otherwise — e.g. `write 0x06=0x02`
  is not produced; the form is `write 0x006=0x02` on Pic14).
  Tracing the same `(address, width)` twice (by name, alias, and
  address in any mix) emits one line, not duplicates.
- Dump (only with `--dump`), printed after the last trace line
  and **before** the final line, byte-stable in this order:
  first the register block, one line each, always in this order —
  `w=0xNN`, `status=0xNN`, `pc=` with the per-core padding
  above; then, for `ram`, one row per 16-byte-aligned block that
  contains a non-zero byte, and for `all`, every block of the
  core's RAM, as `ram 0x0020: 01 02 …` (address `0x` plus 4
  lowercase hex digits, colon, then 16 two-digit lowercase hex
  bytes separated by single spaces; a trailing partial block
  cannot occur since all RAM sizes are multiples of 16); then,
  for `all` only and only on Pic14/Pic14e/Pic18, the EEPROM the
  same way as `eeprom 0x0000: …` over all 256 bytes.
  `PicBaseline` has no `eeprom()`, so `--dump all` on a baseline
  device prints the full RAM and no EEPROM rows — no placeholder.
- Exactly one final line, **always last on stdout** (the dump
  precedes it, never follows it):

  ```text
  halted after 1234 cycles pc=0x0100 w=0x02 status=0x18
  ```

  The first word is the stop reason: `halted`, `breakpoint`, or
  `capped`. `pc=` uses the per-core padding above; `w=` and
  `status=` are two lowercase hex digits from `w()`/`status()`.
- Exit codes, also part of the contract:

  | Code | Meaning |
  |---|---|
  | 0 | The program halted (`halted` final line). |
  | 1 | Runtime error: HEX unreadable or malformed, stimulus file unreadable or malformed (including out-of-range or pseudo-register stimulus pokes, D-5). The diagnostic is on stderr. |
  | 2 | Usage error: unknown flag, bad value, unknown device, unknown SFR name, out-of-range or pseudo-register `--poke`/`--trace-sfr` address, an `interrupt` stimulus line on a PicBaseline device, missing `<hex>`. Usage text follows the diagnostic on stderr. This matches the gdbserver's usage exit. |
  | 3 | The cycle cap was reached (`capped` final line). Distinct from 0 so a tutorial script can tell "ran to completion" from "stopped early". |
  | 4 | A breakpoint was hit (`breakpoint` final line). |

  Note this deliberately differs from the exit sketch in the
  ticket's early comments (which put the cap at 2): 2 is kept for
  usage so both shipped CLIs agree on what a bad invocation costs,
  and the cap and breakpoint get their own codes because scripts
  branch on them.
- `--help` prints usage to stdout and exits 0; `--version` prints
  `epic-cc-sim <EPIC_CC_STAMP>` and exits 0, the gdbserver shape.
- Malformed HEX: the sim parsers panic today (indexing
  `bytes[3]`, `chunk[1]`, bad nibbles, unsupported record types).
  The guarantee that no panic reaches the user rests on one
  mechanism, not on validator completeness: the runner calls the
  parser inside `std::panic::catch_unwind` (with the panic hook
  silenced for that call, restored after) and maps any caught
  panic to exit 1 with `malformed HEX in <file>` on stderr. As
  defence in depth, and to name the offending line in the common
  cases, the runner first validates each record itself: leading
  `:`, even number of hex digits after it, at least 5 bytes
  (count, address, type, checksum), declared byte count equals
  the actual data length, checksum sums to zero mod 256, record
  type is one of 0x00/0x01/0x04, and data length is even for
  data records; the first failing line is reported as
  `malformed HEX in <file>:<line>` with exit 1 without calling
  the parser. A malformed shape that slips past the checklist
  is still caught by the `catch_unwind` boundary (without a line
  number), so the exit-1 promise does not depend on the
  checklist being exhaustive. This follows the repo's stated
  direction that a panic reaching a user is a defect (design 46,
  section 1), scoped to the new binary only; hardening the
  parsers themselves is a separable sim follow-up. The same
  no-panic rule covers stimulus and pokes by construction (D-5
  validation; the runner never calls the asserting
  `read_mem`/`write_mem`).

## 4. How it relates to the existing code

- **`crates/sim` (`pic14-sim`) is the engine, unchanged in v1.** The
  runner consumes the public surface listed in section 1. No new
  sim API is required — the blockers were closed by carving out or
  narrowing the CLI, not by pricing new sim surface into v1:
  stimulus pokes use validated `ram_mut()` writes (not the
  banked/asserting `write_mem`, D-5), `interrupt` stimulus is
  rejected on PicBaseline (which has no hook), tracing is
  change-only comparison of `ram()`/accessor snapshots around
  `step()` (D-4), and the run loop steps per instruction instead
  of needing a cycle/target-set bulk API (D-5). The final line
  and dump use `cycles()`, `pc()`, `w()`, `status()`, `ram()`,
  and `eeprom()` where present — all four cores expose those
  accessors (EEPROM excepted on baseline, handled in section 3).
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
- **No new sim watch or callback API in v1** (D-4), and no cycle/
  target-set bulk-run API (D-5). The watch hook is deferred priced
  work (T6b, S–M, roughly 100–200 lines in `crates/sim`) for when
  a tutorial needs same-value writes such as a TXREG byte stream.
  No change to `pic14-sim` behaviour, the parsers' panic surface
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
| T1: `crates/sim-runner` skeleton | New crate plus root workspace registration: add `crates/sim-runner` to `members` in the root `Cargo.toml` (without it `cargo build -p epic-cc-sim` errors), `build.rs` stamp, hand-rolled `parse_args` with `USAGE`, `--help`/`--version`, device resolution, the four-way dispatch enum (differing `pc`/`write_mem` types, 256–4096 B RAM, EEPROM/interrupt presence), HEX pre-validation plus the `catch_unwind` parser boundary (section 3), the always-stepping run loop to halt or cap, final line, exit codes 0/1/2/3. Unit tests for argument parsing; an integration test running a fixture HEX to `halted`; a malformed-HEX test asserting exit 1 and no panic output. | L (roughly 350–450 lines plus tests) | none |
| T2: Breakpoints, pokes, and stimulus | `--break`, `--poke`, `--stimulus` file parser with up-front validation (D-5 address table), the per-step stimulus/breakpoint checks in the run loop, `request_interrupt` stimulus, exit code 4. Tests: timed poke observed by the program, interrupt stimulus taken on a core that has the hook, `interrupt` on a baseline device exits 2, out-of-range flag poke exits 2 and stimulus poke exits 1, malformed stimulus line exits 1 naming file and line. | M (roughly 250–350 lines plus tests) | T1 |
| T3: SFR trace and dumps | `--trace-sfr` name/alias/address resolution over `Device.sfrs` (canonical display, first-match-by-address, deduplication, pseudo-register and multi-byte rules in D-4/section 3), step-and-compare change-only tracing, `--dump regs|ram|all` in the section 3 byte-stable order with the final line last. Tests: PORTB trace lines in order on a blink fixture, same-value write emits no line, unknown SFR name exits 2, `INDF` trace exits 2, dump formats pinned by golden files. | M (roughly 250–350 lines plus tests) | T1 |
| T4: Bundle packaging | Confirm the root `Cargo.toml` member added in T1 builds in the release context; Dockerfile `release` stage and `release.yml` Windows job build and copy the binary; `check_release_bundle.py` requires it; per-platform smoke runs the shipped runner on the smoke HEX and asserts the final line and exit 0. Record measured binary and zip sizes in the PR. | S | T1 (T2/T3 not required: smoke needs only halt) |
| T5: Tutorial readiness | README quickstart gains the compile-then-simulate flow (user-visible surface, so the README rule applies); a short user doc under `docs/user/` for the runner's flags, output, and exit codes, generated from the same `USAGE` where the docs generator allows. Verify the section 5 walkthrough from a clean directory with only the zip. | S | T4 |
| T6 (conditional): Putchar trap | Only if a tutorial needs text output. Small design note for the magic address, sim or runner support, tutorial shim. Not filed until the need is concrete. | S to M | T3, a requesting tutorial |
| T6b (conditional): Sim watch hook | Only if a tutorial needs same-value writes (e.g. a TXREG byte stream). A write hook in `pic14-sim` fired from each core's `write_f`, runner registration, CLI unchanged. | S to M (roughly 100–200 lines in `crates/sim` plus tests) | T3, a requesting tutorial |

Total for T1 to T5: roughly 900 to 1300 lines including tests,
spread over one L, two M, and two S tickets, plus the packaging
lines. No ticket in T1–T5 touches `crates/driver`, `crates/sim`
internals, or any existing CLI (T6b is the conditional exception,
filed only on need). The review gate and takeoff ritual apply per
PR as usual, and each PR that changes user-visible surface carries
its README note.

## 8. Risks and open questions for the approver

- **Run cost on long runs (accepted, not a requirement to slice).**
  The loop always steps per instruction (D-5), so a run costs one
  dispatch-enum step plus traced-register snapshots per
  instruction, traced or not. At the default 1,000,000-cycle cap
  that is tutorial-scale and fine; `--cycles 100000000` will be
  proportionally slower than a bulk `run` would be, and the docs
  should say so rather than promise speed v1 does not have. A
  bulk fast path (or a cycle/target-set sim API) is follow-up
  work only if profiling demands it, and it must reproduce the
  per-step output exactly.
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
