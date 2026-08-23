# ADR-021 -- Device data provenance and the gputils cross-check

**Status:** Accepted 2026-08-23<br>
**Decides:** `epic-cc#104`<br>
**Parent:** `docs/superpowers/specs/2026-08-23-device-provenance-design.md`

## Decision

* Every device TOML carries a required `[provenance]` table. `build.rs` refuses
  to build without it. `tier = "atdf"` needs `source`, `pack` and `sha256`;
  `tier = "datasheet"` needs `document` and `ticket`.
* `crates/device/tests/gputils_crosscheck.rs` gates every device on every PR:
  `flash_words` via two `gpasm` `org` probes, and the RAM map by comparing
  coalesced ranges against the generic `.lkr`.
* Comparison is union-and-coalesce, never element-wise, because gputils splits
  a flat PIC18 window into nine banks describing the same memory as one TOML
  entry.
* Missing gputils fails rather than skips, with `PIC8_ALLOW_NO_GPUTILS=1` as
  the explicit escape.

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

## Revisit if

* A supported part has no `.lkr`, making the cross-check silently vacuous for
  it. The datasheet provenance tier is the current answer, but a second oracle
  would be better.
* `gpr_ranges_from_lkr` starts missing real geometry. It reads `DATABANK` and
  `SHAREBANK` lines textually and does not evaluate `#IFDEF`/`#ELSE` guards or
  match `ACCESSBANK` lines at all. `p18f4550`'s correct `0x0-0x5F` range comes
  from a `DATABANK` inside a never-taken `_EXTENDEDMODE` branch, while the
  active `ACCESSBANK` line is ignored outright; today's result is right only
  because both branches describe the same physical access RAM. A future device
  whose access RAM is declared solely via `ACCESSBANK` would produce a false
  mismatch, and the parser would need to evaluate the guard or read that line.
