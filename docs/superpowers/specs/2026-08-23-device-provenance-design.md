# Device data provenance -- generated TOML + an always-on gputils cross-check -- Design

**Status:** draft (pending user approval)<br>
**Date:** 2026-08-23<br>
**Parent:** `docs/superpowers/specs/2026-08-22-pic-variants-design.md`, `docs/adr/ADR-019-pic-variants-device-registry.md`<br>
**Scope:** `epic-cc` device registry only. No codegen, `alloc` or `isel` change.<br>
**Tickets:** `epic-cc#104` (this: schema + gate). Generator landed as `#103` (ADR-020), closing `#86`. Prior data bugs: `#88`, `#92`, `#101`.

---

## 1. Goal and non-goals

**Goal:** a wrong number in `crates/device/devices/*.toml` fails a build instead of
silently miscompiling, and adding a device does not require anyone to trust a
hand transcription.

**Non-goals (v1):**

* Verifying `config` fuse fields, `sfrs`, or `interrupt_vectors` against gputils.
  The generic `.lkr` does not describe them. They stay generator-verified only.
* Replacing the datasheet as the human reference. This constrains what may enter
  the tree, not what an engineer reads.
* Any change to how `Device` is consumed. `&'static Device` and every consumer
  signature are untouched.

## 2. Why this exists

The device registry is the only input in the compiler with no oracle. Every
other stage boundary has one: the `gpasm` byte-for-byte cross-check, the
simulator, differential fuzzing, and a diffable text artifact per stage. Device
data is asserted, not derived, and nothing in the repo can contradict it.

It is also the highest leverage place to be wrong. `ram_banks` and `flash_words`
sit upstream of banking, allocation and paging, so an error there does not
produce a loud failure, it produces a compiler that is confidently wrong about
real silicon.

Two incidents came out of that gap:

* `#92` / `#101`: banks 2 and 3 began 16 bytes late each, so 32 of the part's 368
  GPR bytes were unreachable on every PIC14 target. The error came from the old
  Rust consts and `#85` copied it into the 887.
* `#88`: `p16f877a.toml` was edited to `flash_words = 16384` with RAM banks out to
  `0x3FF`. PIC14 addresses data through `FSR` plus `IRP`, a 9 bit space capping
  at `0x1FF`, and `PCLATH<4:3>` selects four pages, so that part cannot exist.
  Every firewall that caught it was then rewritten to accept the invented device.

The second is the important one: the feedback signal available to an agent was
"make the red thing green", every failure named a number, and the number lived in
an editable file that nothing could contradict.

## 3. Empirical ground truth

Measured in the dev image (`gpasm-1.5.2`, gputils data under
`/usr/local/share/gputils/`) before scoping:

* `lkr/16f877a_g.lkr` carries our schema almost field for field:
  `CODEPAGE page0..page3` spans `0x0..0x1FFF` (so `flash_words = 8192`),
  `DATABANK gpr0..gpr3` gives `0x20-0x6F`, `0xA0-0xEF`, `0x110-0x16F`,
  `0x190-0x1EF`, and `SHAREBANK gprnobnk` gives `0x70-0x7F`. It held the correct
  bank 2 and 3 boundaries the entire time the hand written TOML was 32 bytes short.
* `header/p16f877a.inc` exists for every part gputils supports.
* `poppler-utils` (`pdftotext`) is already installed.

Probe behaviour, which decides the mechanism split:

| Probe | Result |
|---|---|
| `org 0x2000` (past flash) | detected: `Warning[220] Address exceeds maximum range ... BADROM_START: 0x2000` |
| `udata 0x200` then `gplink` | links clean; absolute addresses are never validated against `DATABANK` |
| relocatable `udata res N` | largest linkable `N` matches no real bank (80/96), so auto-placement reveals no geometry |

gpasm exits 0 on warnings, so the flash check matches on stderr rather than on
exit status. Making `gplink` allocate per bank would require handing it a linker
script containing the RAM ranges, which is circular: we would be asserting
exactly what we are trying to verify. Hence flash by probe, RAM by parsing.

## 4. Decisions

| # | Decision | Rationale |
|---|---|---|
| D-1 | ATDF generates the TOML; gputils only ever cross-checks | Nothing GPL is copied into the tree, so the licence posture in `AGENTS.md` and ADR-019 is unchanged |
| D-2 | Two tier gate: gputils check hard on every PR, ATDF regeneration hard only where the pack is present | The gputils half needs no network and would have caught both incidents; PR builds stay hermetic and a vendor outage cannot redden an unrelated PR |
| D-3 | Every TOML carries a required `[provenance]` stanza; unattested is refused | A falsified value then needs a forged citation, which is visible in review rather than an invisible edit |
| D-4 | Flash verified by gpasm process probe, RAM geometry by parsing the generic `.lkr` | §3 shows probing cannot cover RAM without circularity, and process invocation is the oracle pattern the repo already uses |
| D-5 | Absent gputils fails rather than skips | A gate that disappears when its tool is missing is not a gate |

## 5. Schema

`build.rs` gains one required table, validated with the same panic-on-bad-TOML
path as the rest of the schema.

```toml
[provenance]
tier    = "atdf"          # atdf | datasheet
source  = "PIC16F877A.atdf"
pack    = "Microchip.PIC16Fxxx_DFP.1.7.162"
sha256  = "..."           # of the .atdf, which is never committed

# tier = "datasheet" additionally requires:
# document = "DS39582C"
# tables   = ["TABLE 2-1: Register File Map"]
# ticket   = "epic-cc#92"
```

Rules enforced in `build.rs`:

* Missing `[provenance]` is a build failure.
* `tier` must be `atdf` or `datasheet`; anything else fails.
* `tier = "atdf"` requires `source`, `pack` and `sha256`.
* `tier = "datasheet"` requires `document` and `ticket`.

`sha256` records which ATDF a TOML was generated from. It is provenance, not a
gate: the `.atdf` is gitignored, so the hash is checked only by the generator's
`--check` in the tier that has the pack.

## 6. The always-on cross-check

Lives in `crates/device/tests/gputils_crosscheck.rs` so it runs under `make test`
and in CI with no new infrastructure, and it iterates `device::ALL` so a new
device is gated the moment it is added.

**Flash.** For each device, assemble two probes with `gpasm`:

* `org <flash_words - 1>` must produce no range warning.
* `org <flash_words>` must produce `Address exceeds maximum range`.

Together these pin the bound from both sides, so both an inflated and a deflated
`flash_words` fail.

**RAM.** Parse the generic `<stem>_g.lkr`, collecting `DATABANK` entries whose
name begins `gpr` and `SHAREBANK` entries, then compare:

* the `gpr*` ranges, in order, against `ram_banks`
* the first unprotected `SHAREBANK` range against `common_ram`

A mismatch reports the expected and actual range per bank, not a bare boolean.

**Locating gputils.** `PIC8_GPUTILS_SHARE` overrides a default of
`/usr/local/share/gputils`. If neither resolves, the test fails with an
explanation unless `PIC8_ALLOW_NO_GPUTILS=1` is set. A part with no `.lkr` is
reported as uncovered and must be `tier = "datasheet"`.

**Core scoping.** PIC18 parts have a flat `DATABANK` layout and one `CODEPAGE`
set; the same two comparisons apply, so no per-core branch is expected beyond
the `gpr*` naming.

## 7. The generator, which has already landed

`#86` closed while this design was being written: `#103` merged
`scripts/gen-device.py` (ATDF ingestion, `--check`) plus `ADR-020`. The generator
half of the plan therefore exists, and §6 stands on top of it rather than beside
it.

Two gaps remain, and they are why `#104` is still the load-bearing ticket.

**The `--check` gate is a no-op on every runner today.** `ci.yml` wraps it in a
fallback that skips when `/opt/microchip/xc8/v4.00/pic/packs` is absent, which is
the case on the GitHub runners. Measured on master run `32656428057`:

```
DFP not installed on this runner, skipping strict check for p16f877a
DFP not installed on this runner, skipping strict check for p16f887
DFP not installed on this runner, skipping strict check for p18f4550
```

So the ATDF tier currently contributes zero PR coverage. That is exactly the
"gate evaporates where it matters" failure D-5 rejects, and it is what the
gputils cross-check in §6 fixes: gputils *is* in the image, so its check cannot
be skipped for lack of a download.

**The generated TOMLs carry no provenance.** `gen-device.py` must be taught to
emit the §5 stanza. That is a small addition to an existing script rather than
new work, but it has to happen for `build.rs` to be able to require the stanza.

The `--check` invocation runs `python3` on the host runner rather than in the dev
image, which reads as inconsistent with "everything runs in docker". It stays on
the host: the DFP, when present, is installed on the runner and not in the image,
so moving the check inside would break the only case where it does any work. The
inconsistency is deliberate and worth a comment in `ci.yml` rather than a fix.

## 8. The datasheet fallback

For a part with neither an ATDF nor a gputils entry, and only then.

`scripts/datasheet-extract.md` is a prompt naming exactly which tables to locate
(register file map, program memory organisation, configuration word) and
requiring output as TOML plus citations. `pdftotext` is already in the image.

The output is a **proposal**. A human confirms it once, it lands as
`tier = "datasheet"` with the document number and a ticket, and the value is
thereafter visibly weaker than a generated one. A subagent never writes a trusted
value, and this path is not in the steady state loop.

## 9. Testing

* `build.rs` schema rules: a TOML with no provenance, a bad `tier`, and a
  `datasheet` tier missing `ticket` each fail the build.
* Cross-check runs across the whole registry.
* Two negative controls prove the gate bites: reintroducing `#101`'s late bank
  starts fails with a per-bank diff, and setting `flash_words = 16384` on
  `p16f877a` fails the flash probe.
* Missing gputils fails unless the escape hatch is set.
* The generator is unit tested against a **synthetic** minimal ATDF fixture we
  author. Microchip's own file cannot be committed.

## 10. Migration and risk

Backfill provenance for the three existing TOMLs. `#92` already cites DS39582C
(877A) and DS41291D (887), so those start at `tier = "datasheet"` until
regenerated from ATDF. `p18f4550` has no document recorded anywhere in the repo
yet, so the migration step must look up and cite its datasheet number rather than
inherit one; that lookup is part of the work, not an assumption baked in here.

**Risk:** the cross-check may fail immediately on `p16f887` or `p18f4550` if their
hand written values disagree with gputils. That is the gate working, but it means
step one is to run the check and read its output before wiring it as a blocker.
Any corrections land in their own commits, as `#101` did, so a moved golden is
traceable to the data change that moved it.

## 11. Rejected alternatives

* **gputils generates the TOML.** Cheapest (zero fetch, already in the image) but
  makes the committed file a derivation of GPL data, which is the boundary
  `AGENTS.md` draws.
* **Fetch the DFP on every CI run and hard gate both checks.** Strongest single
  answer, but adds a network dependency and a cache to every build; the cache is
  an optimization elsewhere in this repo, never a gate.
* **Commit a normalized ATDF extract so both checks run offline.** Strong and
  hermetic, but adds a second per-device file that largely restates the TOML.
* **Treat memory maps as uncopyrightable facts and read any source.** Defensible,
  but makes a legal judgement the repo has deliberately avoided.
* **Probe gplink for RAM geometry.** Falsified in §3: absolute `udata` is not
  validated and relocatable placement reveals nothing.
