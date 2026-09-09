# 38: Device onboarding hardening — any 8-bit PIC part, one file

> **Approval status:** approach and phasing approved by the user on
> 2026-09-09. Written as a follow-up to
> `docs/37-pic-baseline-port-design.md`; the long-term objective
> stated was "add support for ANY PIC 8-bit part as easily as
> possible, ideally a matter of adding a single file." An independent
> review pass ran on 2026-09-09 (per this repo's review-gate norm) and
> found six issues, all corrected in place: a missing `[VERIFY]` tag
> on the PIC17-dismissal and device-count claims in §0 (added below);
> P3's phase dependency listed `P1, P2` when P3's own text only ever
> invokes P2's sweep tool, not P1's per-device wrapper (corrected to
> `P2`); D-1 step 2 claimed the gputils cross-check runs "against just
> that device" when the actual test
> (`crates/device/tests/gputils_crosscheck.rs`) loops over the entire
> `device::ALL` registry with no per-device scoping, meaning the
> wrapper's cost scales with total registry size (corrected, and
> flagged as open question 4 below since it interacts with D-2's
> growth plan); D-4's field list was stale against
> `crates/device/src/lib.rs` (missing `access_bank` and
> `fixed_retval`, both PIC18-only compiler-consumed layout facts added
> after ADR-019's original text) and cited a nonexistent "ADR-019 §4"
> (corrected to point at the right place); D-1 step 4's synthesis rule
> had an unstated dependency on every `locked` field happening to also
> carry a matching `default` (corrected to handle `locked` explicitly);
> D-2's `--sweep` mode was described as if it were a small addition
> when `gen-device.py`'s `main()` currently entangles argparse,
> single-file discovery, and `sys.exit`, so a directory-sweep mode is
> real new scope, not a flag (noted explicitly in D-2).

## 0. The punchline, stated up front

The hard part of "any part" is not architecture, it is arithmetic:
Microchip's own product literature splits every 8-bit PIC part ever
shipped into exactly **four** architecture families — Baseline,
Mid-Range, Enhanced Mid-Range, and PIC18 (Microchip, "8-bit PIC
Microcontroller Solutions," DS-39630D, and the current-catalog
brochure at `farnell.com/datasheets/1523199.pdf`, both agree on this
four-way split and give per-family device counts: 16 baseline, 58
mid-range, 29 enhanced mid-range, 193 PIC18 in the brochure's count).
**[VERIFY]**: these counts are from a single search pass, not
independently refetched and cross-checked against a second source in
this revision. No fifth 8-bit core exists in Microchip's current
lineup, so far as checked. **[VERIFY]**: PIC17 existed historically as
a distinct 8-bit core; it is treated here as out of scope on the
strength of it being absent from the two sources cited above and
long discontinued, matching this repo's existing dsPIC/PIC24/PIC32-
out-of-scope posture — not on a direct check of a Microchip statement
that PIC17 is dropped from the current architecture taxonomy.

This repo already has a backend for three of the four (`pic14`,
`pic18`, `pic14e`, all shipped) and the fourth (`pic-baseline`) is
mid-flight as of `docs/37` (`epic-cc#323`-`#330`). **Once that lands,
every 8-bit PIC core Microchip has ever shipped has a backend.**
Adding a *device* on top of an existing core is already single-file in
the common case, proven twice (`docs/32-adding-a-device.md`: PIC14's
`p16f887` in `#85`/`#90`, PIC18's `p18f2550` in `#224`/`#232`/`#234`).

So "any part, one file" is not a request for new architecture. It is
a request to make Path A (`docs/32`'s term: a new device on an
already-supported core) stop being a six-section runbook a human has
to read and start being a command a human (or a bot) just runs.

## 1. Why device diversity doesn't blow this up

The reason one core's worth of devices can stay single-file no matter
how many parts exist in that family is a scope boundary that already
holds today, just not written down anywhere as a decision: **epic-cc's
device model never encodes peripheral behavior.** `Device` (per
`crates/device/src/lib.rs`, checked directly against ADR-019's
decision-list description) carries `core`, `flash_words`, `ram_banks`,
`common_ram`, `stack_depth`, `interrupt_vectors`, `config` (fuse/
config-word fields), and two PIC18-only compiler-consumed layout facts
added after ADR-019's original text, `access_bank` and `fixed_retval`
— plus an *optional* `sfrs` table that exists only "for the HAL
contract" (ADR-019's decision line), never consulted by `isel`, `asm`,
or `sim`. Every one of those fields is memory-layout or fuse-resolution
data the compiler itself needs; none of it is peripheral register
semantics. UART/SPI/ADC/timer register layouts, peripheral
counts, pin muxing — none of it reaches the compiler. That is
`epic-hal`'s problem, not `epic-cc`'s. This is *why* a family with 193
members (PIC18) is no harder to keep single-file than one with 16
(baseline): the compiler's device model is deliberately blind to
almost everything that actually varies member-to-member within a
family.

**This should be written down as an explicit decision (D-4 below),**
not left as an implicit property nobody stated. If it ever stops
holding — if `isel` starts needing a peripheral fact — that's the
signal this whole plan needs revisiting, not a normal device addition.

## 2. Decisions

### D-1: Collapse the runbook into one command

`docs/32` §2-§6 today is: generate (`gen-device.py`), diff against a
sibling by hand, run the gputils cross-check (`cargo test -p device
--test gputils_crosscheck`), run `scripts/sanity.sh <stem>`, hand-write
an `EPIC_CONFIG` fixture covering every declared field, then a human
sign-off. Every one of the first four steps is mechanical. Propose a
single wrapper, `scripts/add-device.sh <part> --atdf <path> [--pack
<name>]`, that:

1. Runs `gen-device.py` to produce the TOML.
2. Runs `cargo test -p device --test gputils_crosscheck`. **Note:**
   this test (`ram_map_matches_gputils_for_every_device` etc.) loops
   over the entire `device::ALL` registry today, not just the new
   device — there is no per-device scoping mechanism. The wrapper's
   per-addition cost therefore scales with total registry size, which
   matters directly once D-2's sweep starts growing that registry;
   see open question 4.
3. Runs `sanity.sh <stem>`.
4. Synthesizes an `EPIC_CONFIG` string mechanically from the TOML's
   own `config` table: for each field, use the `locked` value if the
   field is locked, else `default` if present, else the first
   enumerated value. Handling `locked` explicitly (not just assuming
   it always has a matching `default`) avoids a synthesis-only false
   failure on a future `locked`-without-`default` field. Compiles a
   throwaway fixture against the resulting string.
5. Prints one of exactly two things: "ready to commit" with the
   diffed field list against the closest existing sibling (still
   useful context for review, now computed instead of hand-done), or
   a failure list naming exactly which step failed and why.

**What stays manual, deliberately:** sourcing the DFP itself.
Microchip's pack downloads are account-gated and the packs are
gitignored on purpose (ADR-020); automating that fetch is either a
scraper (fragile, licensing-adjacent) or a stored credential
(security-adjacent) for a step that takes a human under a minute.
Not worth it.

### D-2: Prove breadth with a sweep, not one device at a time

The alias tables in `gen-device.py` (`FIELD_ALIAS`, `PART_FIELD_ALIASES`,
`PART_VALUE_ALIASES`) have been exercised on 10 devices across 3
families. A DFP pack for PIC18Fxxxx alone covers on the order of the
193-device count cited in §0; PIC14/PIC14E's combined packs are
larger still. Real coverage will surface DFP spelling quirks the same
way `p18f2550` vs `p18f4550` already did once (`div1`/`osc1_pll2`,
`boren`/`bor`, per `docs/32`'s pitfalls table). The fix in every case
so far has been a data-only alias-table addition, never a core code
change — but "any part" should mean that's discovered by a deliberate
sweep across a whole pack, not by whoever tries device #47.

Proposed sweep tool: point `gen-device.py` (in a new `--sweep <pack-dir>`
mode) at an entire unpacked DFP directory and have it attempt
generation for every part inside, collecting exceptions into a triage
report (unknown field name, unknown value, missing `DCRDef`, etc.)
instead of stopping at the first failure. **This is real new scope,
not a small flag addition**: `gen-device.py`'s current `main()`
entangles argparse, single-file source discovery, and `sys.exit` calls
for the single-part case; a directory sweep needs its own
per-part-from-directory discovery path and needs `generate_toml`'s
existing exception handling (`MissingFacts`) threaded through a loop
instead of a single `try`. Run it once per family against the largest
available pack as a proof, starting with PIC18Fxxxx since it's the
biggest family and already has two shipped exemplars to validate the
sweep tool itself against.

**Not proposed:** committing every part the sweep can generate. A
generated-but-uncommitted TOML from a sweep has not been gputils
cross-checked or sanity-tested per device (D-1's checks are real work,
`cargo test`/`gpasm` time per part); the sweep's job is to find
generator gaps cheaply, not to pre-populate the registry.

### D-3: Generation stays on-demand, not bulk

Matches the standing norm already used for baseline itself
("revisit if a real consumer wants one," `docs/37`'s approval header):
don't commit TOMLs for parts nobody has asked for. D-1 makes the
on-demand path fast (one command, minutes) and D-2 makes it reliable
(known-working alias tables) — between them, "on-demand" stops meaning
"read a runbook" and starts meaning "run a command," which is the
actual goal. Bulk-generating hundreds of never-verified TOMLs would
create a maintenance surface (drift, unverified provenance) with no
consumer benefiting from it yet.

### D-4: Make the peripheral-blindness boundary an explicit, written decision

State outright, in `docs/08-status-and-next-steps.md`'s device-registry
bullet and in `ADR-019` as an addendum: `Device`'s schema is closed
over "what the compiler needs to place code and data and resolve
config fuses," and peripheral register semantics are permanently out
of that schema — `epic-hal` owns them. This is not a new restriction;
it is what ADR-019 already built (the `sfrs` table is optional and
HAL-only) and what has made every device addition single-file so far.
Writing it down turns an accidental property into a decision someone
would have to deliberately revisit, the same posture as the
dsPIC/PIC24/PIC32 scope boundary already gets in `docs/08`.

## 3. Phases

| # | Phase | Depends on |
|---|---|---|
| P0 | Docs-only: state D-4 explicitly in `docs/08` + an ADR-019 addendum; confirm the four-core closed set in `docs/08`'s device-registry bullet with the Microchip citation from §0 | Nothing |
| P1 | `scripts/add-device.sh`, the one-command wrapper (D-1) | P0 |
| P2 | `gen-device.py --sweep`, the breadth-proofing tool (D-2) | P0 |
| P3 | Run the sweep against the PIC18Fxxxx pack (largest family, two existing exemplars to validate against); fix whatever it finds in `gen-device.py`'s alias tables only | P2 |
| P4 | Repeat the sweep for PIC14 and PIC14E packs | P3 |
| P5 | Repeat the sweep for the baseline pack, once `pic-baseline` has at least one shipped device (`epic-cc#323`) to validate the sweep tool against | P3, epic-cc#323 |
| P6 | Retire `docs/32`'s six-section runbook language in favor of "run `add-device.sh`"; keep the pitfalls table (still useful context), demote the step-by-step to what the script does internally | P1-P5 |

## 4. Open questions

1. Should `add-device.sh`'s synthesized `EPIC_CONFIG` fixture use a
   single legal value per field, or try to exercise every enumerated
   value combinatorially? A single pass is cheap and catches the
   "unknown value" failure shape `docs/32` §5 already documents; full
   combinatorial coverage is more thorough but may be disproportionate
   for a fixture whose only job is "does resolve_config panic."
   Leaning toward single-value-per-field for P1, revisit if it misses
   something real.
2. Does every part in a family's DFP pack actually have a
   corresponding `gpasm` `.inc` oracle, or only the ones already
   commonly used? If gputils coverage is partial, D-1's cross-check
   step needs a defined behavior for an oracle-less part (the existing
   `PIC8_ALLOW_NO_GPUTILS`/`PIC8_UNVERIFIED_DEVICE_DATA` opt-out from
   `docs/32` §3 already covers this shape; confirm it's the right
   answer here too, not a new mechanism).
3. P5 depends on baseline shipping at least one real device — should
   this document block on that, or is `docs/37`'s own `p12f509.toml`
   (its P0, `epic-cc#323`) enough to unblock P5 without waiting for
   the full P0-P8 port to finish? Leaning toward the latter: the sweep
   tool only needs a TOML to validate against, not a working backend.
4. The gputils cross-check re-verifies the entire `device::ALL`
   registry on every run (no per-device scoping exists today, see
   D-1 step 2). As D-2's sweeps grow the registry, `add-device.sh`'s
   per-addition wall-clock cost grows too. Worth a per-device-scoped
   cross-check mode once the registry is large enough for this to be
   felt, but not blocking P1 — flagging so it isn't rediscovered as a
   surprise later.
