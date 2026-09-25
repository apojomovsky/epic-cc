# 46: Public beta through PlatformIO

> **Approval status:** draft, awaiting the user's approval. The scope
> answers (audience, host OSes, board set, programmers, XC8
> conventions, tool redistribution) were given by the user during
> brainstorming on 2026-09-25; the rest are proposals marked as such.
> Spans three repos (epic-cc, epic-platformio, and a new `epic8-tools`)
> and supersedes, for the beta, the "every device, no curation" board
> decision in epic-platformio's `docs/platform-decisions.md`.

**Goal:** a quiet public beta a hobbyist reaches from the PlatformIO
registry: `platform = <owner>/epic8`, `pio run`, `pio run -t upload`
with a programmer they already own, on Linux. Code written the way PIC
tutorials write it compiles unchanged.

**Definition of done:** each beta board builds its examples and is
flashed and run on silicon with each programmer that can physically
reach it (section 4, D-11), from a clean Linux machine that has only
PlatformIO installed.

---

## 1. Where things stand

Established by inspection on 2026-09-25.

| Gap | Evidence |
|---|---|
| No register headers: `PORTB`, `TRISBbits.TRISB0` are undeclared without epic-hal. `xc.h` is an empty stub. | `crates/driver/src/xc_h.rs` |
| The device TOMLs carry config fields but no SFR map, and config names are normalized (`osc`, `wdt`), not the pack's (`FOSC`, `WDTE`). | `crates/device/devices/p16f1937.toml` |
| Headers are written to a per-run temp dir, so an IDE cannot resolve `epic-cc.h` or `stdint.h`. | `crates/driver/src/main.rs:120` |
| The toolchain package declares `"system": "*"` but ships Linux x86_64 only, and there is no `platform.py` to pick per host. | epic-platformio `packages/toolchain-epiccc/package.json`, `platform.json` |
| Only 3 of 115 boards carry `upload`, `f_cpu` and memory sizes; generated boards cannot flash or report size. | epic-platformio `boards/p16f1937.json` vs `boards/p16f877a.json` |
| `pio run -t size` is a stub although the driver has a report. | epic-platformio `builder/main.py` `_size`, `crates/driver/src/report.rs` |
| The pk2cmd hint points at `cjacker/pk2cmd-minus`, idle since 2023; the live upstream is `jaka-fi/pk2cmd` (kair.us). | GitHub API, 2026-09-25 |
| A panic reaches the user as a Rust panic, backtrace hint included. | driver `main.rs` |
| The platform is 0.0.1, unpublished; its README still says upload is not wired. | epic-platformio `platform.json`, `README.md` |

## 2. Decisions

### D-1: Audience and hosts

Hobbyists first. Linux x86_64 for the beta, Windows x86_64 next, macOS
out of scope. Rationale: every packaging choice below is per host, and
one host keeps the hardware sign-off matrix small enough to finish.

### D-2: A curated beta board set

`pic16f877a`, `pic16f887`, `pic16f628a`, `pic12f675`, `pic16f1937`,
`pic18f4550`. These cover all three production cores (PIC14, PIC14E,
PIC18), the parts hobbyists actually stock, and the two
programming hazards worth exercising (18F4550's LVP pin, 12F675's
calibration words).

Board ids follow PlatformIO's convention of the chip's own name
(`pic16f877a`, not `p16f877a`); `build.mcu` keeps epic-cc's device name.
The remaining generated boards stay in the repo under
`boards-experimental/`, documented as copy-into-your-project files
(PlatformIO reads a project-local `boards/` natively), so nothing is
lost and nothing unverified is advertised.

**Amends** epic-platformio's "no curation" decision, which assumed every
listed board is equally usable; the beta makes a support promise per
board, which a generated list cannot back.

**Rejected, ship all 115 with a visible tier field.** `pio boards` has no
place to show a tier, so a user sees 115 equal-looking boards.

### D-3: XC8 source conventions, generated from pack data

Tutorial code compiles unchanged. The surface, all in the beta:

| Surface | Delivery |
|---|---|
| SFR names and `<REG>bits.<BIT>` unions | Generated per-device headers, dispatched from a real `<xc.h>` on the device macro |
| `_XTAL_FREQ`, `__delay_ms()`, `__delay_us()` | `xc.h` macros over a cycle-exact delay intrinsic |
| `void __interrupt() isr(void)` (and `high_priority`/`low_priority` on PIC18) | `xc.h` macro onto the existing interrupt section spelling |
| `#pragma config NAME = VALUE` | Driver lowers it into the same resolution `EPIC_CONFIG` uses |

The data comes from the Microchip device packs, which are Apache-2.0
(ADR-029): the ADR-020 generator already reads the `.PIC` files; it
grows an SFR/bitfield table and keeps the pack's native config field
and value names as aliases of the normalized ones. Headers ship with the
pack's attribution. `p16f877a` is `tier = "datasheet"` with no pack, so
its SFR table and aliases are transcribed from the datasheet, same as
its config data.

`#pragma config` needs the driver because clang drops unknown pragmas
before the `.ll`. Mixing it with `EPIC_CONFIG` in one program is an
error, not a merge.

**Proposed:** stop predefining `__XC8` and define `__EPIC_CC__`. Code
that tests `__XC8` usually goes on to use surface we do not have
(`__at()`, `__eeprom`, `__section`), so the predefine promises too
much. The m-stack sources that need it get `-D__XC8` in their own build.

**Rejected, epic-cc-only spellings.** Every tutorial, book and forum
answer uses these names; a hobbyist would have to translate before the
first blink.

### D-4: Config bits in the source are the single source of truth

The driver already derives the system clock from the config
(`crates/driver/src/fosc.rs`). Proposed:

- `board_build.f_cpu` becomes optional; when set, the build fails if it
  disagrees with the derived clock.
- A user `#define _XTAL_FREQ` that disagrees with the derived clock is a
  warning, since tutorial code always defines it.
- The build header prints the result: `PIC16F877A @ 4 MHz (HS)`.

Rationale: config travels with the code and builds the same outside
PlatformIO; two sources that must agree always eventually disagree.

### D-5: Distribution through the registry under a personal owner

- `platform.py` maps the host to a toolchain package
  (`linux_x86_64` now, `windows_amd64` in the Windows phase), the same
  shape as Community-PIO-CH32V, and pulls a programmer tool package
  only when a project selects that protocol.
- Platform, toolchain and framework are published under the user's
  personal registry account, versioned `0.x` for the beta.
- A compatibility table (platform to toolchain to framework) ships in
  the platform README; the platform pins exact package versions.

### D-6: Headers live on disk in the toolchain package

The toolchain package ships `include/` next to the binary. The driver
resolves it exe-relative (the same chain as clang discovery, doc 30
D-6) and materializes to a temp dir only as the fallback. The builder
adds it to `CPPPATH`, so PlatformIO's generated IntelliSense config
finds every header.

### D-7: Build UX

- **Size bar.** The builder reads the driver's report and feeds
  PlatformIO's program-size check. Board `maximum_size` stays in bytes
  (two per word, as HEX addresses are), and the header line states words.
- **Diagnostics.** Errors print as `file:line:col: error:` so editors
  link them.
- **Internal errors.** A panic hook prints `epic-cc: internal compiler
  error: <message>`, the version, and the issue URL with what to attach
  (source, `--version`); backtraces only under `RUST_BACKTRACE`. Panics
  stay the error surface (AGENTS.md); only their presentation changes.

### D-8: An upload layer with per-tool device names

Protocols in the beta: `minipro` (XGecu T48, TL866II Plus), `pk2cmd`
(PICkit2, PICkit3 and "3.5" clones), `picpro` (K150 and siblings,
firmware protocol P18A), and `custom` (`upload_command`). `upload_flags`
passes through on every protocol.

Boards carry `upload.devices.<tool>` (the name each tool expects) and
the package facts the checks in D-9 need (DIP availability, LVP
default, calibration words). Targets: `upload`, `erase`, and
`readback` (dump flash to a HEX file).

### D-9: Programming hazards are warned, not refused

The builder warns before flashing when:

- the target is a DIP-only socket programmer (K150) and the board's
  package is not DIP;
- the config leaves LVP on for a part whose PGM pin then needs a
  pull-down (18F4550 RB5, 877A RB3);
- the part stores calibration words in flash (12F629/675 OSCCAL and
  bandgap) and the tool is not known to preserve them;
- MCLR is disabled with the internal oscillator, so recovery needs a
  Vpp-before-Vdd programmer.

None of the beta programmers is LVP-only, so there is no lockout case
to refuse; that rule arrives with the first LVP-only protocol.

### D-10: `epic8-tools`, pinned upstream plus a patch queue

A new repo builds the programmer tools as PlatformIO tool packages from
**pinned upstream tags plus a small patch directory**, not long-lived
forks: patches are sent upstream so the queue stays short, which is
what keeps maintenance cheap. CI builds per host and publishes to the
registry; each package ships its licence and source reference.

| Tool | Upstream | Licence | Redistribution |
|---|---|---|---|
| `tool-minipro` | gitlab.com/DavidGriffith/minipro, 0.7.x | GPL-3.0 | Binary plus the exact source tarball in the release. libusb (LGPL-2.1) bundled as a shared library, not linked statically. |
| `tool-pk2cmd` | github.com/jaka-fi/pk2cmd | Microchip PK2CMD licence | Clause 1(b) permits distributing modified versions for use with Microchip products, provided a fixed copyright and "modified by" notice is prominently shown. The package README and the tool's banner carry it. `PK2DeviceFile.dat` ships with it. |
| `tool-picpro` | github.com/Salamek/picpro | GPL-2.0 | Pure Python, run under PlatformIO's own interpreter, dependencies vendored with their licences. |

Invoking these as separate processes keeps them outside the compiler,
as with gpasm today (AGENTS.md GPL boundary). The pk2cmd reading above
is the project's, not legal advice; the notice is the mitigation, and
bring-your-own `EPIC8_PK2CMD_PATH` stays available regardless.

### D-11: Hardware sign-off gates the beta

The user flashes and runs each board's blink example with each
programmer that can reach it (K150 and T48 socket: DIP parts; T48 ICSP
and PICkit: all). A board whose row is green gets `"support":
"hardware"` in its JSON and a row in the support table; the others
ship as `"simulator"`. All software work in sections 3 and 4 lands
before the hardware arrives, so sign-off is the last step, not a
blocker mid-way.

### D-12: Out of scope for the beta

macOS; the simulator-backed `pio debug` and `pio test` (PIC14 only
today, a later headline feature); epic-hal as PlatformIO libraries
(the framework stays first-class); programmers the user does not own
(a-p-prog, pickle, pymcuprog), reachable meanwhile through `custom`.

## 3. Work breakdown

Tickets are filed on the epic8 board after approval. Arrows are hard
dependencies.

**epic-cc**

| # | Work | Depends on |
|---|---|---|
| CC-a | DFP generator emits SFR/bitfield tables and pack-native config aliases; 877A transcribed | |
| CC-b | Generated device headers and a real `xc.h` (SFRs, bits, `_XTAL_FREQ`, delays, `__interrupt()`); `__EPIC_CC__` replaces the `__XC8` predefine | CC-a |
| CC-c | `#pragma config` lowering in the driver | CC-a |
| CC-d | On-disk `include/` in the bundle, exe-relative resolution | |
| CC-e | Clock consistency checks (`f_cpu` flag, `_XTAL_FREQ`) and the header line | |
| CC-f | Internal-error panic hook and `file:line:col` diagnostics | |
| CC-g | Machine-readable size report the builder can parse | |
| CC-h | Release v0.4.0 carrying all of the above | CC-a..g |

**epic8-tools** (new repo)

| # | Work | Depends on |
|---|---|---|
| T-a | Repo skeleton, patch-queue build, CI publishing to the registry | |
| T-b | `tool-minipro` (T48 verified in its device list) | T-a |
| T-c | `tool-pk2cmd` with the licence notice | T-a |
| T-d | `tool-picpro` | T-a |

**epic-platformio**

| # | Work | Depends on |
|---|---|---|
| P-a | `platform.py`: per-host toolchain, on-demand tool packages | |
| P-b | Curated boards (ids, `upload.devices.*`, sizes, hazard facts), `boards-experimental/`, generator update | |
| P-c | Upload layer: `picpro` and `custom` protocols, `erase`/`readback`, `upload_flags` | P-b, T-b..d |
| P-d | Hazard warnings | P-b |
| P-e | Size bar, `CPPPATH` for IntelliSense, clock line | CC-h |
| P-f | udev rules (PICkit, T48, K150 serial) and per-programmer guides | P-c |
| P-g | Examples in tutorial style (no HAL) and with the HAL, per beta board | CC-h |
| P-h | README and getting-started refresh, compatibility table, registry publication of platform, toolchain, framework | all above |

**Sign-off:** D-11 matrix on silicon, then the beta announcement.

**Windows phase:** toolchain and tool packages for `windows_amd64`,
USB driver notes (WinUSB via Zadig for PICkit and T48).

## 4. Open items

- Whether each beta part's pack SFR data needs the datasheet cross-check
  the config data got in doc 38, or whether pack-tier trust is enough.
- The exact PK2CMD notice text and where the banner shows it, fixed
  when T-c lands.
