# ADR-020 -- DFP -> TOML generator (ATDF/EDC ingestion)

**Status:** Accepted 2026-08-23<br>
**Decides:** `epic-cc#86`<br>
**Parent:** ADR-019, `docs/superpowers/specs/2026-08-22-pic-variants-design.md` section 9<br>
**Scope:** `crates/device` TOML authoring only; no compiler change

## Decision

* `scripts/gen-device.py` is the one-shot generator that reads Microchip DFP data and emits `crates/device/devices/<stem>.toml` deterministically. Primary input is the DFP's `xc8/pic/dat/{ini,cfgdata}` (the same data that ships inside the `.atpack` zip) and its `edc/*.PIC` XML. The raw `.atpack`/`.atdf`/`.PIC` itself is never committed, only the TOML it generates.
* An explicit `--atdf` XML may be passed, but the default path finds the packs under `$PIC8_XC8_ROOT/pic/packs` (or `/opt/microchip/xc8/v4.00`). When XC8 is not installed the script explains where to fetch the pack from `https://packs.download.microchip.com/` (e.g. `Microchip.PIC16Fxxx_DFP`) and exits 2.
* The provenance `pack` name is a directory-level fact of the `.atpack` layout, not something the XML states. It is derived from the source file's nearest `*_DFP` ancestor directory; a file extracted outside its pack directory needs `--pack <name>` passed explicitly, and the generator refuses to write the stanza when neither resolves, never `pack = "unknown"` (ADR-021 posture).
* `gputils` headers are the byte-for-byte oracle, XC8 headers are black-box oracle only, per `AGENTS.md` GPL boundary. The generator documents this posture in its header.
* Config field and value names are normalised to the `EPIC_CONFIG` names via a small alias table documented in the script header (`FOSC` -> `osc`, `WDTE` -> `wdt`, `INTRC` values -> `intosc_*`, etc.). The alias table is the only hand-maintained mapping; everything else is derived.
* Output is deterministic: fields sorted by `byte_offset` then `shift`, values by `bits`, hex with leading zeros, one field per `[[config.fields]]` block. `python3 scripts/gen-device.py PIC16F887 --out crates/device/devices/p16f887.toml` round-trips the hand-written TOML. `scripts/gen-device.py --check` plus `git diff --exit-code` in CI guarantees the committed TOML matches the DFP.

## Rationale

Hand-transcribing a datasheet table is correct once but scales as `O(devices)`. The DFP is the authoritative machine-readable source for the same facts (RAM banks, flash size, config words). Using it removes the transcription tax at device #3+ while keeping the TOML as the reviewed, diffable artifact. Not committing the `.atpack`/`.atdf` keeps the repository licence-clean, same posture as config transcription today.

## Alternatives rejected

* **Commit the .atdf/.PIC** -- would bloat the repository and raise licence questions about redistribution of Microchip pack contents. Rejected: the TOML is the derived, licence-clean artifact.
* **Parse only gputils .inc** -- `__MAXRAM`/`__BADRAM` is reliable for RAM but config bits are scattered across many `EQU` lines without a single mask/value table. The `cfgdata` file is a purpose-built table for config with mask, value, and alias lists, so it is the better parser input. `gputils` remains the oracle that the generator can be cross-checked against.

## Consequences

* Adding a same-ISA part is now `python3 scripts/gen-device.py <stem> --out crates/device/devices/<stem>.toml` plus review of the TOML diff. No Rust edit.
* `p16f877a.toml` was regenerated to the same canonical ordering as `p16f887.toml` (values sorted by bits, `wrt` names normalised to `half`/`1fourth`/`256`/`off` per `cfgdata`). The diff from the previous hand-written `p16f877a.toml` is an intentional alias fix, not a semantic change.
* CI nightly or per-PR `--check` will fail if the TOML drifts from the DFP, with a unified diff pointing to the field that changed.

## Revisit if

A new core requires fields not in the schema or a new pack layout breaks the `xc8/pic/dat/{ini,cfgdata}` assumption -- extend the script's search, not the manual transcription path.
