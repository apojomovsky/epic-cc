# ADR-012: CC-3 silicon-real codegen: clock derivation, HEX emission, and fixes

**Status:** Accepted 2026-08-21 (implemented in feat/cc3-silicon-codegen)

## Decision

Ship `epic-cc.h`, per-device config-word tables, a multi-region HEX writer, and a driver-side `EPIC_FOSC_HZ` derivation that make a program boot real silicon (D-8/D-9/D-10):

* `EPIC_AT(addr)` and `EPIC_CONFIG("...")` as `section` attributes forwarded verbatim by clang 20.1.8; `irparse` reads `.epicat.<hex>` into `Global.addr`, the driver reads `.epiccfg.<spec>` after the `llvm-link` merge.
* Config tables live in `device` as `FuseField`/`ConfigRegion` (ADR-004), resolved by `resolve_config` starting from the device's `erased_baseline`. `XINST` is `locked=Some("off")`.
* `to_hex_regions` emits a new `:04` extended-linear-address record only when the upper 16 bits change; PIC14's config word at byte `0x400E` (word `0x2007`, DS39582C Register 14-1) and PIC18's region at `0x300000` (DS39632E Table 25-1) both go through it, `to_hex` itself untouched.
* `EPIC_FOSC_HZ` is a `-D` macro added before any clang invocation, derived by a comment/string-literal-aware pre-scan (`prescan::find_epic_config`) and a clock model read from the vendored datasheets (DS39582C §14.2, DS39632E §2.2 / Registers 25-1/25-2).

## EPIC_FOSC_HZ arithmetic actually shipped

`xtal_hz=<Hz>` is not a fuse; it is stripped before `resolve_config` (see `fosc::split_xtal_hz`) and is the only non-silicon input to the frequency. `resolve_fosc_hz` validates the fuse half via `resolve_config` first, so locked/missing-field panics come from one path.

* **PIC16F877A (DS39582C §14.2.1):** LP/XT/HS/RC are the four `osc` encodings; none has a PLL or a postscaler. `EPIC_FOSC_HZ = xtal_hz`. Missing `xtal_hz` panics naming it and the required section.
* **PIC18F4550 (DS39632E §2.2 / Register 25-1, byte 0x300000):** `osc` decides the tree, `PLLDIV` and `CPUDIV` the divisors, `USBDIV` is orthogonal.
  * `inths|intxt|intcko|intio`: internal-oscillator microcontroller modes: `8_000_000` (INTOSC). `CPUDIV` does not apply (Register 25-1: it applies to XT/HS/EC and to the PLL modes).
  * PLL modes (`hspll|xtpll|ecpll|ecpio`): PLL produces a fixed `96 MHz` from a fixed `4 MHz` input (§2.2.4). `PLLDIV` (`noprescale|div2|div3|div4|div5|div6|div10|div12`) must satisfy `xtal_hz / factor == 4_000_000` (otherwise panic naming `xtal_hz` and `plldiv`). `CPUDIV` (`div1|div2|div3|div4`) maps to `96 MHz / 2,3,4,6` respectively (Register 25-1: `00=÷2, 01=÷3, 10=÷4, 11=÷6` in PLL modes). So `EPIC_FOSC_HZ = 96_000_000 / pll_cpu_div`.
  * Non-PLL crystal modes (`hs|xt|ec|ecio`): `CPUDIV` (`div1|div2|div3|div4`) maps to `xtal_hz / 1,2,3,4` (Register 25-1: `11=÷4, 10=÷3, 01=÷2, 00=÷1`).
* **No-EPIC_CONFIG path:** `resolve_fosc_hz_from_defaults` returns `0`, the inert value from `epic-cc.h`'s `#ifndef EPIC_FOSC_HZ` guard. Existing fixtures have no `EPIC_CONFIG` and must keep compiling; an empty fuse spec would otherwise panic on the required oscillator field.

Validated by five unit tests in `crates/driver/tests/fosc.rs` covering XT direct, HSPLL→48 MHz, HS÷2, and the two panic paths, plus the `config_probe` e2e fixture (`osc=xt, xtal_hz=4000000`) that boots the simulator at `0x0021`.

## HEX and simulator fixes discovered while wiring

* **`asm::to_hex_regions` trailing-zero trim.** The plan's single-chunk parity test (`[0x2830,0x0064,0x0000]`) exposed that `to_hex` trims trailing zero words while the first `to_hex_regions` draft did not, producing `:060000...` vs `:040000...`. Fixed to trim per-chunk tail to `hi = rposition(|w| w!=0)`; config chunks are `0xFF`-erased (`0xFFFF` words) so they are never trimmed, only a program image's zero tail is.
* **`sim::parse_hex` buffer panic (Task 6 Step 7).** `parse_hex` allocated `vec![0u16; 8192]` and wrote `words[addr/2+i]` for a PIC16F877A config word at word `0x2007` (8199), out of bounds. Fixed to a two-pass sizing: first pass computes `max_word = max(8191, max addr/2+len/2)`, second writes. Existing 8192-word fixtures unchanged, the `config_probe` e2e needs it.
* **`parse_hex_pic18` gap (Known gaps).** `0x04 => {}` discards the window value, so a `to_hex_regions` chunk at `0x300000` would alias onto low addresses. Not fixed in this work; the e2e only exercises the PIC14 path. Fix by tracking the current upper window the same way `parse_hex` now sizes its buffer.
* **`alloc` now honors `Global.addr` (EPIC_AT).** The plan wired `irparse` → `Global.addr` but left `alloc` placing every non-const global sequentially from `gpr_start`. `config_probe`'s `out EPIC_AT(0x0021)` would have collided with `fosc` at `0x20`. Fixed to pin `addr:Some` globals first, bump the sequential cursor and the bin-pack cursors past any fixed-occupied range, and derive `end_of_globals` from the final map.

## Verification gaps corrected during this work

* **`gpasm_config` byte address for PIC14.** The plan's `bytes_at(&hex, 0x000E)` assumed the PIC14 config word at byte `0x400E` would appear as `0x000E` in the HEX data record's low 16 bits. `gpasm` emits `:02400E00...`: low 16 is `0x400E` (extended address is `0x0000`), the PIC18 case `0x300006 → 0x0006` remains the only one where the upper window matters. Fixed to `0x400E`.
* **Shared `~/.cache/epic-cc/target` contamination.** With several worktrees active, `make test` can show phantom `cannot find function` failures for tests that exist only in this branch. Verified the true pass with an isolated `CARGO_TARGET_DIR` as the plan's §"Things to know" prescribes; `scripts/ci-test.sh` under `docker run` shows the full suite green.

## Revisit if

A future device needs a clock tree that is not `xtal_hz` plus the divisors above, or `parse_hex_pic18` is needed for a PIC18 config-word e2e (fix the window tracking then).
