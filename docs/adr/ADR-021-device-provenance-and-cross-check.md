# ADR-021 -- Device data provenance and the gputils cross-check

**Status:** Accepted 2026-08-23<br>
**Decides:** `epic-cc#104`<br>
**Parent:** `docs/superpowers/specs/2026-08-23-device-provenance-design.md`

## Decision

* Every device TOML carries a required `[provenance]` table. `build.rs` refuses
  to build without it. `tier = "atdf"` needs `source`, `pack` and `sha256`;
  `tier = "datasheet"` needs `document` and `ticket`.
* `crates/device/tests/gputils_crosscheck.rs` gates every device on every PR:
  `flash_words` via two `gpasm` `org` probes, and the RAM map against the
  generic `.lkr`.
* On PIC14 the comparison is per bank and ordered, plus a separate check of
  `common_ram` against the first unprotected `SHAREBANK`. A merged total would
  accept a moved banked/common boundary, which `isel` reads as `fsr_window`.
  Union-and-coalesce survives only on PIC18, where gputils splits a flat window
  into nine `DATABANK`s that one TOML entry describes.
* Coverage is correlated with provenance: a device with no `.lkr` is named in
  the test output and must be `tier = "datasheet"`. Nothing may be both
  unverified and silent.
* Missing gputils fails rather than skips. The escape is two variables,
  `PIC8_ALLOW_NO_GPUTILS` plus `PIC8_UNVERIFIED_DEVICE_DATA`, and prints a
  banner; one variable alone still fails. Both are read only when the data root
  is absent, so they cannot silence a gate that could have run.
* `scripts/gen-device.py` refuses to emit a TOML for a field its source does
  not state, naming the field and exiting non-zero. It has no per-device
  fallback constants: a generated map that carries a real `sha256` reads as
  attested, so a guess there is worse than no generator.

## Rationale

Device data was the only input with no oracle, and it sits upstream of banking,
allocation and paging, so an error there miscompiles silently. `#92` and `#101`
lost 32 bytes of GPR to a hand transcription; `#88` widened the part to hardware
that cannot exist and then rewrote every firewall that objected. The ATDF check
from ADR-020 cannot cover this on a PR, because it skips when the DFP is absent,
which is every GitHub runner. gputils ships in the dev image, so its check
cannot be skipped for want of a download.

## Alternatives rejected

* **Generate the TOML from gputils.** Cheapest, but makes the committed file a
  derivation of GPL data, which is the boundary `AGENTS.md` draws.
* **Fetch the DFP on every CI run.** Adds a network dependency to every build;
  a cache is an optimization in this repo, never a gate.
* **Probe gplink for RAM geometry.** Measured and rejected: absolute `udata` is
  never validated, and relocatable placement reveals no bank geometry. Supplying
  a linker script with the ranges would assert what we are verifying.

## Known exception

`p18f4550` ships `common_ram = [0x0,0xF]` while its `.lkr` access RAM is
`0x0-0x5F`, so the two cannot be compared. On PIC18 `common_ram` is not the
hardware access window: it is `isel-pic18`'s fixed, `BSR`-independent retval
and scratch reservation carved out of the bottom of access RAM, a compiler
choice no linker script can attest. PIC18 therefore compares only the total
allocatable span, and the banked/common boundary stays unverified there. A
per-device `access_bank` field, which `docs/29` originally sketched and the
registry collapsed into `common_ram`, would make it checkable.

## Revisit if

* A supported part has no `.lkr` **and** gpasm does not know its `-p` name.
  The RAM half then reports it uncovered and demands the datasheet tier, but
  the flash half fails outright because the probe cannot run. That is the
  right failure and not a silent pass, though it means such a part cannot be
  added without a second oracle.
* The `.lkr` guard evaluation stops matching how we assemble. `ram_from_lkr`
  evaluates `#IFDEF`/`#ELSE`/`#FI` with no symbol defined, which is true of
  epic-cc (no gputils C runtime, no extended instruction mode) and selects the
  `ACCESSBANK accessram` arm on the 4550. A part built only with
  `_EXTENDEDMODE` on would need the guards driven by a symbol set.
