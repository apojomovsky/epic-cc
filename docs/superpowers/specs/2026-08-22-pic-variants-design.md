# PIC Variant Support -- file-per-device TOML + canonical-per-core CI -- Design

**Status:** draft (pending user approval of sections below)<br>
**Date:** 2026-08-22<br>
**Parent:** `docs/31-ecosystem-integration-design.md` D-2/D-4, `docs/03-decisions.md` ADR-004, `docs/29-pic18-port-design.md` D-3<br>
**Scope:** `epic-cc` compiler only. HAL side is a contract section deferred to `epic-hal#70`.<br>
**Tickets:** `epic-cc#83` (registry), `#84` (CI), `#85` (p16f887 exemplar), `#86` (DFP→TOML follow-up); HAL `#70`/`#71`.

---

## 1. Goal and non-goals

**Goal:** adding a same-core PIC (e.g. `p16f887` alongside `p16f877a`) is a file add, not a Rust edit, and CI does not grow as `devices × fixtures`. A contributor who knows a datasheet can ship a new part; the compiler change is data.

**Non-goals (v1):**

* `pic14e` -- `p16f1937` and the enhanced mid-range core (49 extra insns, linear GPR, `MOVLP`/`ADDFSR`) need a new backend `isel-pic14e`, not a TOML. `core = "pic14e"` is a loud firewall.
* Per-device SFR header emission in `cc` beyond an empty `sfrs` table -- that is HAL's job (#70).
* Goal-directed clock solver sugar over `EPIC_CONFIG` (doc 31 D-4 deferred). Names stay datasheet-names.
* Full `devices × fixtures` fuzz/matrix. By construction a bug is core-wide or data-wide (see §7).

---

## 2. Empirical ground truth

* `crates/device` already has two profiles `PIC16F877A: Pic14` and `PIC18F4550: Pic18` with `ram_banks`, `common_ram`, `flash_words`, `stack_depth`, `interrupt_vectors`, `ConfigRegion`. `alloc`, `banking`, `sim`, `asm`, `driver`, `fuzz` thread `&Device`.
* Remaining hardcoding: `isel::fsr_window` enumerates 4 PIC14 windows `[0x20,0x80) [0xA0,0xF0) [0x120,0x170) [0x1A0,0x1F0)` literally; `STATUS,7` IRP bit, `0x70-0x7F` common RAM literals appear in comments/tests.
* `16F887` is same ISA as `16F877A`: 35 insns, same `STATUS.RP1:RP0` banking, same 4×2K paging, single `FSR+IRP`, 8-deep stack, 368 B GPR in same banks, `0x70-0x7F` common, `8K×14` flash, single vector `0x0004`. Only config changes (877A: 1 word at `0x2007`; 887: 2 words `0x2007/0x2008` with ~18 fields) plus SFR additions (`ANSEL/ANSELH/OSCCON`). Verified against DS39582C vs DS41291D.
* Source posture per `AGENTS.md`: GPL tools as process invocation are fine, transcribing tables into source is a different act; XC8 is black-box oracle only.

---

## 3. Approaches considered

### A -- File-per-device TOML + `build.rs` codegen (recommended)

Each device is `crates/device/devices/<stem>.toml`. `build.rs` globs them, validates, emits `OUT_DIR/devices.rs` with `pub const` blocks + `ALL` + `by_name()`. `lib.rs` `include!`s it. Adding a device touches zero hand-written Rust.

*Pros:* contributor mental model (one TOML per part), `git diff --name-only` gives "changed devices" for CI, `ALL` needs no hand edit, schema is the validator, `serde` gives precise errors. Matches ADR-004's sketch but codegen'd rather than runtime-parsed, so `Device` stays `&'static` and every consumer keeps its `&Device` signature. *Cons:* one `build.rs`, one schema, one nightly `gen-device --check` to keep TOMLs honest.

### B -- Hand-written Rust consts (what we have now)

Stay with two `pub const` blocks in `lib.rs`, add a third by hand for each part.

*Pros:* zero tooling, compiler is the validator, no build script. *Cons:* every addition edits a Rust file, central registry needs hand-edit, `git diff`-based "changed devices" is fragile, reviewer must hold Rust syntax. Rejected as the scaling path; retained as the *fallback* for the first two seed devices before A lands.

### C -- Runtime-parsed TOML (`include_str` + `toml::from_str` at startup)

Driver loads `devices/*.toml` at process start, builds owned `Device` values.

*Pros:* no codegen. *Cons:* `Device` would contain `String`/`Vec`, forcing `&Device` to become owned or `Cow` through `alloc`/`isel`/`sim` -- every callsite changes for no benefit, and compile-time errors become runtime errors. Rejected: A gives the same "file per device" UX with compile-time guarantees.

**Chosen: A.** B remains the fallback until A lands; C is not taken.

---

## 4. TOML schema

```toml
# crates/device/devices/p16f887.toml -- illustrative, not normative values
name = "p16f887"
core = "pic14"           # "pic14" | "pic18" | "pic14e" (pic14e = firewall, no backend yet)
flash_words = 0x2000
ram_banks = [[0x20, 0x6F], [0xA0, 0xEF], [0x120, 0x16F], [0x1A0, 0x1EF]]
common_ram = [0x70, 0x7F] # or null for PIC18 Access-Bank parts (then bank_of returns None for that range)
stack_depth = 8
interrupt_vectors = [0x0004] # PIC14: [0x0004]; PIC18: [0x0008, 0x0018]

[config]
base_byte_addr = 0x400E   # = word 0x2007 *2; 887 second word at 0x4010 (word 0x2008*2) -- verify
num_bytes = 4
erased_baseline = [0xFF, 0xFF, 0xFF, 0xFF]

[[config.fields]]
name = "fosc"
byte_offset = 0
mask = 0x07
shift = 0
default = "intosc_noclkout"
locked = null            # Some("off") for XINST-only fields, etc.
values = [{name="lp", bits=0b000}, {name="xt", bits=0b001}, {name="hs", bits=0b010}, {name="intosc_noclkout", bits=0b100}]

# optional SFR table for HAL contract -- empty initially, consumed by hal#70
[[sfrs]]
name = "ANSEL"
addr = 0x088
width = 1
fields = [{name="ANS0", mask=0x01, shift=0}, {name="ANS1", mask=0x02, shift=1}]
```

**Invariants (enforced by `build.rs`):**

* `ram_banks` sorted by `lo`, non-overlapping, `lo <= hi`, all `hi <= 0x7FF`.
* `common_ram` disjoint from every bank (and from SFR holes -- banks already encode the holes by being separate ranges).
* `flash_words` > 0, power-of-two-ish (warn if not 0x1000/0x2000/0x4000 etc.).
* `erased_baseline.len() == num_bytes`.
* Every `field.mask` has contiguous bits aligned to `shift`; `values[*].bits` fits within mask width.
* `interrupt_vectors` length matches `core` (1 for pic14, 2 for pic18 with IPEN, etc.).
* `core == "pic14e"` panics at codegen with `"no backend for core pic14e (need isel-pic14e)"` -- firewall.

File naming: `<name>.toml` where `<name>` is the canonical `Device.name` (`p16f877a`, `p16f887`, `p18f4550`). Driver's `--target` resolves via `by_name` on that stem.

---

## 5. `crates/device` design

**From:**

```rust
pub const PIC16F877A: Device = Device { name: "p16f877a", core: Core::Pic14, flash_words: 0x2000, ram_banks: &[...], common_ram: Some((0x70,0x7F)), ... };
pub const PIC18F4550: Device = Device { ... };
impl Device { pub fn region_for(&self, addr: u16) -> Option<(u16,u16)> { ... } pub fn bank_of(&self, addr: u16) -> Option<u8> { ... } }
```

**To:**

```
crates/device/
  devices/
    p16f877a.toml
    p18f4550.toml
  build.rs          # glob + serde + validate + emit OUT_DIR/devices.rs
  src/lib.rs        # include!(concat!(env!("OUT_DIR"), "/devices.rs")); hand-written impl Device + ConfigRegion types unchanged
  tests/device.rs   # schema tests (validate each TOML, negative: overlapping banks must fail)
```

`build.rs` steps:

1. `glob("devices/*.toml")` → for each file: `toml::from_str` into a private `DeviceToml` (with `Vec<(u16,u16)>` etc.).
2. Validate invariants above (panic with `file:line:field` precise message).
3. Emit `OUT_DIR/devices.rs` containing, for each `stem`:
   ```rust
   pub const PIC16F877A: Device = Device { name: "p16f877a", core: Core::Pic14, flash_words: 0x2000, ram_banks: &[(0x20,0x6F), ...], common_ram: Some((0x70,0x7F)), stack_depth: 8, interrupt_vectors: &[0x0004], config: ConfigRegion { base_byte_addr: ..., num_bytes: ..., erased_baseline: &[0xFF,0xFF], fields: &[...] }, sfrs: &[] };
   ```
   plus `pub const ALL: &[&Device] = &[&PIC16F877A, &PIC18F4550, ...];` and `pub fn by_name(s: &str) -> Option<&Device> { ALL.iter().find(|d| d.name==s).copied() }`.
4. `println!("cargo:rerun-if-changed=devices")`.

`impl Device` (`region_for`, `bank_of`, `gpr_start`) stays hand-written and generic over `ram_banks`/`common_ram` -- no PIC14 literal.

---

## 6. De-hardcoding

* **`isel::fsr_window`**: replace the 4 literal windows with derived windows from `Device::ram_banks` + `common_ram`. The four GPR windows are exactly the GPR banks minus the `common_ram` hole in the first bank; compute them once per compilation and cache.
* **`sim`**: memory map construction iterates `Device::ram_banks` rather than a hard-coded `0x20-0x6F` etc. PIC14's SFR hole handling stays -- it's the complement of `ram_banks` plus `common_ram`.
* **`asm`**: `MAXRAM`/`flash_words` bound already comes from `Device`; no literal change needed beyond audit.
* **Tests/fixtures** with `list p=p16f877a` remain -- they are gpasm cross-check golden files, not compiler config; the compiler's device is now `--target`.

---

## 7. Driver

* New flag: `--target <name>` (alias `-mcu <name>`) -- string, not `Target` enum, resolves via `device::by_name`. `PIC8_DEVICE` env remains for `fuzz` harness.
* Unknown name: `panic!("unknown target '{name}', available: {}", ALL.iter().map(|d| d.name).join(", "))`.
* PlatformIO contract: `epic-platformio/boards/<stem>.json` will carry `"mcu": "<name>"`, builder does `epic-cc --target ${BOARD_MCU} sources...`. No driver-side board table needed.
* Config reporting stays: resolved config words printed (bytes) as today, now for whichever `Device.config` was selected.

---

## 8. CI stratification -- why linear is wrong and what replaces it

A compiler bug is either **core-wide** (wrong `isel` for a C construct -- fails on one `pic14` iff it fails on every `pic14`) or **data-wide** (wrong TOML value -- caught by schema + one sanity compile for that device). Running `devices × fixtures × fuzz` on every PR is the HAL trap.

| Gate | When | What |
|------|------|------|
| **Canonical per core** | Every PR, always | `p16f877a` (`pic14`) + `p18f4550` (`pic18`): full 15 e2e fixtures → HEX → `gpasm -p <canonical>` byte-match + `sim` + `fuzz` seeded 200 + `config` resolve. Fixed 2-job matrix. |
| **Per-device lightweight** | PR iff `devices/*.toml` touched; nightly always | For each touched device: schema/invariants, `alloc` empty-prog, one 80-B global placement check, `asm` flash bound, single `add.c → asm/hex/gpasm -p <device>` round-trip. ~0.5s/device. No fuzz, no float/const tortures. |
| **Nightly** | `schedule: cron` + `workflow_dispatch` | Lightweight for *all* devices in `devices/*.toml`. Proves no TOML drifted. |
| **Never** | -- | Full per-device `devices × 15` e2e, full per-device fuzz. A fuzz bug is core-wide by construction; the canonical already found it. |

Implementation: `ci.yml` splits into `canonical` (fixed matrix) and `devices-changed` (detects `git diff --name-only origin/master -- crates/device/devices/*.toml`, loops). `make sanity DEVICE=<stem>` helper for local. `docs/05-verification.md` updated.

---

## 9. DFP → TOML generator (follow-up #86, not slice 1)

* Input: Microchip DFP `*.atdf` (e.g. `PIC16F887.atdf` from PIC16F1xxxx_DFP), gitignored, fetched by the script. Fallback: `/opt/microchip/xc8/v4.00/pic/include` is black-box oracle only.
* Output: deterministic `devices/<stem>.toml`.
* `scripts/gen-device.py <stem> --out devices/<stem>.toml` + `--check` (`git diff --exit-code`) in CI nightly to gate drift.
* Source posture: ATDF primary (authoritative, free download), `gputils` `.inc` as byte-for-byte oracle, XC8 headers black-box only (AGENTS.md GPL boundary). ATDF field/value names are normalised to our `EPIC_CONFIG` alias table (`WDTE`→`wdt`, `FOSC` values etc.) documented in the script header.
* Not required for `#83`/`#85` -- hand-transcribed TOMLs are the proof that the schema is honest.

---

## 10. HAL contract

`cc` owns memory map + config (the compiler needs them); `hal` owns SFR headers + peripheral bodies. The only shared artifact is the TOML `sfrs` table (initially empty, populated when `hal#70` starts). `hal`'s generator may read the same ATDF or the same TOML `sfrs` table to emit `pic16f87xa-hal/include/generated/<stem>.h`; `cc` consumes no SFR names at codegen. This split is why two ADRs are needed (cc ADR here, hal ADR in `epic-hal`).

---

## 11. PlatformIO integration

`epic-platformio` PIO-1 (#3) will carry `boards/<stem>.json` with `"mcu": "<stem>"` and a `framework-epichal` package that bundles the generated `hal` headers. The builder does `epic-cc --target ${BOARD_MCU} ${SOURCES}` in one whole-program invocation (doc 31 D-7). No `cc`-side board table: `--target` is the interface.

---

## 12. Sequencing and dependencies

Single crate-ordered pass; cross-crate contract is the `Device` shape and `--target` string.

1. `crates/device` -- `build.rs` + two TOMLs + schema tests.
2. `crates/isel` + `crates/sim` -- de-hardcode `fsr_window`/map from `Device`.
3. `crates/driver` -- `--target`/`-mcu` + `by_name` + listing.
4. `ci.yml` -- split canonical vs devices-changed (can be same PR as (1) or a follow-up #84; not required for the exemplar).
5. `PIC16F887.toml` (#85) -- validates "file add = new device" without touching Rust.

No parallel crates needed beyond the obvious `device` vs `driver` boundary; keep PRs slice-shaped (#83, then #84, then #85, then #86).

---

## 13. Testing

* `device` unit: every TOML round-trips through `by_name`, `region_for`/`bank_of` agree with `ram_banks`, negative overlapping-banks TOML fails with precise message.
* `alloc`/`sim`/`asm` still parameterized by `&Device`; no test changes needed beyond the seed devices now coming from TOML (existing `PIC16F877A` references become `by_name("p16f877a").unwrap()`).
* `driver` e2e: unknown target lists `ALL`; `epic-cc --target p16f887` sanity `add.c` committed HEX via golden path; `gpasm -p p16f887` oracle for that HEX.
* CI: PR with only `p16f887.toml` touched proves `devices-changed` ran once, not 15 × 3.

---

## 14. Risks

* **Bad TOML** -- mitigated by schema + invariants + `gpasm` oracle. Not by running every fixture per device.
* **`pic14e` mistaken for `pic14`** -- firewall panic is the error surface (panics-are-the-error-surface rule).
* **ATDF licence** -- `.atdf` itself not committed, only the TOML it generates; derivation is a transcription, not a copy, same posture as today for config bits.

---

## 15. Not in v1

* `pic14e` backend, `XINST`, goal-directed clock solver, per-device SFR header emission, DFP generator.
