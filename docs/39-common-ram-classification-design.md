# 39: `common_ram` classification for PIC14/PIC14E devices without a documented shared corner

Status: **proposal, awaiting sign-off** before any phase ticket is filed.
Decides: `epic-cc#393`'s two open questions, split into a decision D-1 is
ready to commit to and a survey D-2 leaves as a follow-up call for the
person who picks it up.

## 0. Problem statement, restated more precisely than the filing issue

`epic-cc#393` was filed after `epic-cc#387`/`#395` hit
`crates/isel/src/lib.rs`'s and `crates/isel-pic14e/src/lib.rs`'s
`device.common_ram.expect("isel's fixed scratch/retval/ISR-save layout
needs a common-RAM region")` panic on 6 real, popular devices: PIC16F74,
PIC16F84 (from `#387`), PIC12F629, PIC12F675, PIC10F320, PIC10F322 (from
`#395`).

The issue framed this as purely an *ISR* problem ("the ISR prologue needs
a bank-independent byte... no such address exists"). That undersells the
actual code: both `select_with_locs` functions compute `(common_lo,
common_hi) = device.common_ram.expect(...)` **unconditionally**, before
even checking `has_isr` — `common_ram` also backs the fixed `scratch`
byte (used by every multi-byte compare/shift/multiply sequence) and the
4-byte `retval` region (used by every function call with a return value).
So today, a device with `common_ram = None` cannot compile *any* PIC14/
PIC14E program, interrupt or not. Any fix or non-goal has to account for
this, not just the ISR case.

## 1. Why `common_ram` is `None` on all 6: the classifier is working as
   designed, and it is telling the truth — but the truth has two shapes

`scripts/gen-device.py`'s bank classifier (function starting near line
612) builds `common_ram` from `GPRDataSector` entries that carry a
`shadowidref` (real GPR shadowed a second time under a different bank
number) **only when that shadow's own bank also has a primary sector of
its own** (`banks_with_primary`). A bank that is *entirely* one shadow —
no primary sector at all — "votes nothing" per the code comment: its
target bank does not become a `common_ram` candidate, it just collapses
into a smaller `ram_banks` list with the shadow bank removed.

Checking the six devices' real DFP `GPRDataSector` declarations
(`/tmp/dfp-cache/packs/.../*.PIC`, `Microchip.PIC16Fxxx_DFP` and
`Microchip.PIC10-12Fxxx_DFP`) sorts them into two structurally different
buckets:

### Bucket 1: the entire GPR is one physical region, reachable regardless of any bank-select bit — 5 of 6 devices

| Device | Declared banks | Physical shape |
|---|---|---|
| PIC16F84 | bank0 `0x0C-0x4F` (primary), bank1 `0x0C-0x4F`-equivalent (100% shadow, no primary) | one 68-byte region, addressable via bank0 or bank1 |
| PIC12F629 | bank0 `0x20-0x5F` (primary), bank1 `0xA0-0xDF` (100% shadow of bank0, `shadowidref="gpr0"`) | one 64-byte region, addressable via bank0 or bank1 |
| PIC12F675 | identical shape to PIC12F629 | one 64-byte region |
| PIC10F320 | bank0 `0x40-0x7F` only — **no bank1 `GPRDataSector` exists at all** | one 64-byte region, no aliasing even declared, just a single bank |
| PIC10F322 | identical shape to PIC10F320 | one 64-byte region |

For every one of these, `bank_of()` would (if asked) resolve **every**
reachable bank-select-bit combination to the exact same physical bytes —
there is no *other* region a stray bank value could land on. That is a
stronger property than the classifier currently extracts anything from:
the entire region is bank-independent in the strongest possible sense,
not just partially. This matches the classic, well-documented PIC16F84
"shared RAM" fact: a 1990s reference implementation for exactly this chip
(phanderson.com's `EXT_INT2.ASM` walkthrough) places its interrupt
`W_SAVE`/`STATUS_SAVE` at `0x0C` specifically **because** bank0 and bank1
of the F84 are the same 68 bytes twice over, and states this is safe from
any bank state — independent confirmation the hardware fact is real, not
a DFP artifact.

The reason the classifier still emits `common_ram = None` for these is a
schema gap, not a wrong reading of the silicon: its `common_candidates`
rule is looking for a *mixed* bank (some primary GPR plus a shadow into
another bank's real corner — PIC16F628A/87/88/747/877's `gprnobnk`
shape), which correctly identifies a *small corner* of a bank as
bank-independent. It has no rule for the degenerate case where the
*entire* GPR is that shape, because a shadow-only bank (no primary
sector) never enters `banks_with_primary` at all.

### Bucket 2: two real, physically distinct regions — 1 of 6 devices (PIC16F74)

PIC16F74's DFP states 4 banks: bank0 `0x20-0x7F` (primary, real silicon),
bank1 `0xA0-0xFF` (primary, **real, distinct silicon — no
`shadowidref`**), bank2 (100% shadow of bank0), bank3 (100% shadow of
bank1). Unlike bucket 1, there genuinely are two different physical
96-byte regions; `STATUS<RP1>` selects between them for real (`RP0` is
what collapses/is unused, since each region already spans a full
128-byte page). No address exists that resolves to the same byte
regardless of `RP1`. This is the shape the filing issue accurately
described as having "no correct answer to fall back to" for a
common-RAM-style single fixed address.

**Nothing else excluded so far falls into a third shape** (a real
mixed-bank sub-corner classifier miss); these two buckets account for
all six.

## 2. D-1 (decided, ready for a phase ticket): give the generator a rule for bucket 1

When, after collapsing full-bank shadows, `ram_banks` collapses to
**exactly one physical region** and no `common_ram` was found, that
single region *is* bank-independent in full — carve a fixed-size window
off one end of it for `common_ram`, leaving the remainder as `ram_banks`
for ordinary global allocation. This needs **zero changes** to
`isel`/`isel-pic14e`/`schedule`/`sim`/`banking`: they already treat
`common_ram` and `ram_banks` as ordinary disjoint windows (`build.rs`
already asserts they don't overlap), so once the TOML states the split,
everything downstream Just Works.

**Sizing and placement, matching the existing convention exactly:**
every already-shipped PIC14 device with a real `common_ram` corner
(`p16f628a`, `p16f877a`, `p16f887`, ...) places it as the **last 16
bytes** of its region (`0x70-0x7F`, immediately below the next bank's
SFR page). Applying the same rule (top 16 bytes of the sole region) to
all 5 bucket-1 devices:

| Device | Full region | Proposed `common_ram` | Remaining `ram_banks` |
|---|---|---|---|
| PIC16F84 | `0x0C-0x4F` (68 B) | `0x40-0x4F` | `0x0C-0x3F` (52 B) |
| PIC12F629 | `0x20-0x5F` (64 B) | `0x50-0x5F` | `0x20-0x4F` (48 B) |
| PIC12F675 | `0x20-0x5F` (64 B) | `0x50-0x5F` | `0x20-0x4F` (48 B) |
| PIC10F320 | `0x40-0x7F` (64 B) | `0x70-0x7F` | `0x40-0x6F` (48 B) |
| PIC10F322 | `0x40-0x7F` (64 B) | `0x70-0x7F` | `0x40-0x6F` (48 B) |

PIC10F320/322 land on the *exact same absolute address* (`0x70-0x7F`) as
every later mid-range device's common RAM — not a coincidence, this is
Microchip's own convention for where common RAM sits in a mid-range
address map; these two small XLP parts simply don't have any *other*
GPR bank to distinguish it from. 16 bytes clears `isel`/`isel-pic14e`'s
current fixed layout (`scratch` 1 + `retval` 4 + `isr_save` up to 9 = 14
bytes, plus each function's own trailing free-byte assert) with one byte
of headroom identical to the shipped devices' margin; a phase
implementing this should re-read the live constants rather than trust
this doc's arithmetic.

**Also do, per `epic-cc#393`'s "also needed" section, in the same
phase:** relax `crates/device/build.rs`'s `"{core} requires common_ram"`
panic (currently hard for pic14/pic14e/pic-baseline) back to accepting
`None` — confirmed already in the issue that nothing downstream assumes
`Some` except the two `isel*` crates' own `.expect(...)`, which stay as
informative panics for whichever devices still legitimately have none
(bucket 2, until/unless D-2 resolves it; genuinely bankless single-bank
devices that don't need the split at all).

This is an ADR-worthy schema decision (`docs/adr/ADR-034`); it is not
itself a `common_ram`/`ram_banks` code change today, so nothing in this
pass touches `gen-device.py`.

## 3. D-2 (surveyed, not decided): bucket 2 has a real fix, but it's backend work, not a generator tweak

For PIC16F74's shape (and any future device the wider 1007-part catalog
might share it with — untested in this pass), the classic technique for
context-saving on a mid-range PIC with **no** common-RAM byte is
real and precedented, not folklore:

1. **Save W first, into a per-region shadow slot at a fixed low offset
   that exists validly inside every physical region.** PIC14's direct
   addressing computes the physical address as `{RP1:RP0} ++
   opcode<6:0>` at *execution* time, so a single fixed instruction like
   `MOVWF 0x20` always lands in "whatever region is currently selected,
   at offset `0x20`" — it needs no prior knowledge of which region that
   is. For PIC16F74 specifically, region A (`0x20-0x7F`) and region B
   (`0xA0-0xFF`) share the same low 7 bits at that offset (`0x20` vs.
   `0x20|0x80=0xA0`), so the *same* opcode is safe from either starting
   region. This needs the allocator to reserve that offset out of
   *every* region's normal allocation pool, not just one.
2. **Capture `STATUS` non-destructively, force a known region, and stash
   `STATUS` there:** `SWAPF STATUS, W` (documented not to touch any
   status bits, unlike `MOVF`) puts the original `RP1:RP0` (and
   `Z`/`DC`/`C`) into `W` without disturbing them; `BCF STATUS, RP1` /
   `BCF STATUS, RP0` then forces region A unconditionally (safe with no
   prior bank knowledge, since writing `STATUS` bits is never itself
   bank-dependent); `MOVWF` into a plain, ordinary, region-A-only fixed
   byte (no bank-independence needed here at all — it's an ordinary
   `ram_banks` byte, reached because the code just forced that exact
   region) finishes the capture.
3. **Restore is the mirror image, and needs a second swap, not a direct
   write-back.** Force region A again (idempotent if the ISR body already
   left it there), then `SWAPF` the saved byte back into `W` (undoing
   step 2's swap — a single swap is not its own inverse, so writing the
   once-swapped byte straight to `STATUS` scrambles `RP1:RP0` and the
   arithmetic flags into the wrong nibble instead of restoring them; this
   project's own already-shipped `common_ram`-backed ISR epilogue uses
   the identical swap-to-capture/swap-to-restore pair for exactly this
   reason), then `MOVWF STATUS` from `W`, which atomically restores
   `RP1:RP0` and the flags and simultaneously re-selects whichever region
   was active at interrupt entry — so the very next instruction can
   safely read back the per-region `W` shadow from step 1, because the
   region is now provably right again. `RETFIE`.

   Confirmed by direct simulation, not just by inspection:
   `crates/sim/tests/interrupts_no_common_ram.rs` hand-assembles both this
   corrected sequence (passes: W, STATUS's flags, and the bank are all
   exactly restored across a disruptive ISR that clobbers all three) and
   the single-swap version this section originally described (fails
   exactly as predicted, kept as a regression test of the failure mode).

This is the same idiom the classic (bucket-1) PIC16F84 tutorial cited in
§1 uses, generalized to two *non-aliased* regions instead of one; it is
Microchip's own documented answer to "no common RAM," not something this
project would be inventing.

**Revised cost estimate, superseding this section's original one:** the
same simulation pass traced `crates/banking`'s actual mechanics
(`crates/banking/src/lib.rs`'s module doc) and found it is a separate
pass over `isel`'s raw assembly output that inserts `BANKSEL` generically
from `Device::bank_of(addr)` for *any* file-register operand — it has no
idea a given address is "the retval region" versus an ordinary local.
`retval_lo`/`scratch`/the ISR save area skip banking today only because
`bank_of` (`crates/device/src/lib.rs`) explicitly exempts `common_ram`
and `fixed_retval`. Relocating them into an ordinary `ram_banks` region
instead makes `bank_of` return `Some(bank_index)` for them, and the
existing, already-shipped banking pass then inserts correct `BANKSEL`s
at every one of their ordinary (non-ISR) use sites with **no changes to
`isel`'s ~148 existing `retval_lo`/`scratch` call sites** — not the
across-the-board explicit-`BANKSEL`-everywhere rewrite this section
originally predicted. The only genuinely new fact needed is the W-shadow
slot's own banking exemption (mirroring `common_ram`/`fixed_retval`) plus
the new ISR prologue/epilogue codegen itself, which is what
`epic-cc#393`'s implementation lands.

## 4. Alternatives considered and rejected

* **Treat the whole bucket-1 region as `common_ram`, none of it as
  `ram_banks`.** Rejected: `crates/alloc` never places ordinary globals
  in `common_ram` (it is reserved exclusively for `isel`'s fixed
  scratch/retval/ISR-save layout), so a program would compile with zero
  bytes available for its own variables. The split in §2 is the version
  of this idea that actually works.
* **One documented non-goal covering all six devices.** Rejected now
  that the DFP evidence shows 5 of the 6 have a real, near-zero-risk fix
  available; writing off bucket 1 to match bucket 2's harder case would
  be leaving working silicon support on the table for no reason.
* **Solve D-2 by proving no code path leaves a non-default bank state
  live across an interrupt-enabled region, so the ISR can assume a known
  bank.** Rejected as a direction: the `banking` pass has no whole-program
  bank-liveness analysis today (it works locally, eliding redundant
  `BANKSEL`s within a tracked run), and building one to serve exactly one
  device's interrupt support is a much larger investment than the
  shadow-slot technique in §3, which needs no such analysis.

## 5. Consequences

* D-1, once implemented, unblocks PIC16F84, PIC12F629, PIC12F675,
  PIC10F320, PIC10F322 for full Path-A onboarding (interrupts included),
  closing 5 of the 6 `epic-cc#393` exclusions.
* `build.rs`'s `common_ram` relaxation (§2) is safe to land independent of
  D-1's TOML changes — it only removes a panic for a case that no shipped
  device currently exercises turning `Some`-only into `Option`-honest,
  per the issue's own grep.
* PIC16F74 (and any structurally identical device the wider catalog turns
  up later) stays excluded from `crates/device/devices/` until D-2 is
  separately decided and, if chosen, implemented.
* No code in `gen-device.py`, `isel`, `isel-pic14e`, `build.rs`, or any
  device TOML changes in this pass — this document is the decision D-1
  needs before a phase ticket writes that code.

## 6. Open questions for whoever picks up the D-1 phase ticket

* Confirm `isel`'s and `isel-pic14e`'s *exact* current byte counts (this
  doc used 14 + 1 free-byte headroom read from the source at investigation
  time; re-derive rather than trust a doc that can drift).
* Decide whether the generator should carve the split automatically
  (a new, tested rule in `gen-device.py`'s classifier) or whether each of
  the 5 devices gets a manual, `docs/32`-§3-style cited correction the way
  `p18f2550`'s RAM defect was hand-fixed — the automatic rule is less
  repetitive but is exactly the kind of heuristic ADR-021 already warns
  against the generator inventing without a source fact backing it
  (there is no DFP field saying "reserve the top 16 bytes"; it is this
  project's own convention, not Microchip's data).
