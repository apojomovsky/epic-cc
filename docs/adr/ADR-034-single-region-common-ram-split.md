# ADR-034 -- Devices whose entire GPR collapses to one physical region get a carved-out `common_ram` corner

**Status:** Proposed 2026-09-11 (awaiting human sign-off before a phase
ticket is filed)<br>
**Decides:** `epic-cc#393` D-1<br>
**Parent:** `docs/39-common-ram-classification-design.md`, ADR-020, ADR-021

## Decision

* When `scripts/gen-device.py`'s bank classifier collapses a device's
  `GPRDataSector` entries (after removing full-bank shadow aliases) down
  to exactly **one** physical region, and no other bank-independent
  corner was found, that single region is bank-independent in full: every
  reachable value of the device's bank-select bits resolves to the same
  physical bytes, because there is no *other* region for a stray value to
  land on. The top 16 bytes of that region become `common_ram`; the
  remainder stays `ram_banks`, matching the byte count and top-of-region
  placement every already-shipped device with a real `common_ram` corner
  already uses (`p16f628a`, `p16f877a`, `p16f887`, ...).
* `crates/device/build.rs`'s `"{core} requires common_ram"` panic (pic14,
  pic14e, pic-baseline) relaxes from a hard requirement back to accepting
  `None`. Confirmed by grep across `isel`, `isel-pic-baseline`, `sim`,
  `report`, `schedule`: none of them assume `Some`; only `isel`'s and
  `isel-pic14e`'s own `.expect(...)` calls do, and those stay as
  informative panics for whichever devices genuinely still have `None`.
* This rule is generator/schema-level only. `isel`, `isel-pic14e`,
  `schedule`, `sim`, and `banking` need no code changes: they already
  consume `common_ram`/`ram_banks` as ordinary disjoint windows.

## Rationale

`docs/39` §1 found the classifier's existing `common_candidates` rule
(a shadow whose own bank *also* carries a primary GPR sector) correctly
identifies a small mixed-bank corner as bank-independent
(`PIC16F628A`/`87`/`88`/`747`/`877`'s `gprnobnk` shape) but has no rule
for the degenerate case where a bank is *entirely* a shadow with no
primary sector anywhere to mix with — voting nothing, per the code's own
comment. Five real devices (`PIC16F84`, `PIC12F629`, `PIC12F675`,
`PIC10F320`, `PIC10F322`) hit exactly this gap: their whole GPR is one
physical region (verified against each device's real DFP
`GPRDataSector` declarations), stronger bank-independence than the
mixed-bank case already handles, just not expressed by any existing
rule.

## Alternatives rejected

* **Declare the entire single region as `common_ram`.** `crates/alloc`
  never allocates ordinary globals out of `common_ram` (`docs/39` §4);
  a program would have nowhere to place its own variables.
* **A documented non-goal instead**, matching `docs/35`'s shape.
  Rejected: the fix has no known correctness risk and costs zero
  `isel`/`schedule`/`sim`/`banking` changes, unlike bucket 2's
  (`PIC16F74`'s) genuinely harder case, which `docs/39` §3 leaves as a
  separate, undecided follow-up rather than folding it in here.

## Consequences

* Unblocks Path-A onboarding (interrupts included) for `PIC16F84`,
  `PIC12F629`, `PIC12F675`, `PIC10F320`, `PIC10F322` — 5 of the 6 devices
  `epic-cc#393` excluded.
* The 16-byte, top-of-region placement is this project's own convention,
  not a fact any DFP field states (`docs/39` §6 flags this explicitly for
  whoever implements it): decide in the implementing phase whether the
  generator applies this automatically (new, tested classifier rule) or
  each of the 5 devices gets a manual, `docs/32`-§3-style cited TOML
  correction. This ADR does not decide that implementation-shape question.
* `PIC16F74` and any structurally identical future device (two or more
  genuinely distinct physical GPR regions, none of them common) are
  unaffected by this ADR; they stay excluded until `docs/39` §3 (D-2) is
  separately decided.

## Revisit if

* The wider 1007-part catalog turns up a device whose classifier
  collapse produces one region but whose free space, after a 16-byte
  carve-out, is too small for any real program (would need a
  device-specific override of the split size, not a change to this rule).
* A future DFP revision starts stating an explicit "shared RAM" field for
  single-bank/fully-aliased devices, making the 16-byte convention a
  source fact instead of this project's own choice.
