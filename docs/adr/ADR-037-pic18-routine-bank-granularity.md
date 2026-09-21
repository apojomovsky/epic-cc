# ADR-037 -- PIC18 runtime-routine frames round on the BSR bank, not a region

**Status:** Accepted 2026-09-21<br>
**Decides:** `epic-cc#509`<br>
**Parent:** `docs/adr/ADR-031-float-frames-like-integer.md` (whose rationale
this ADR corrects)

## Context

`alloc`'s `routine_base` exists to keep a runtime routine's whole frame
inside one bank, because the recipes are skip-sensitive: a `MOVLB` between
`BTFSC`/`BTFSS` and the instruction it skips, or inside a same-skip carry
idiom (`INCFSZ f,W` then `ADDWF g,F`), changes what the skipped instruction
does. It decided "one bank" with `Device::region_for`, an `ram_banks` entry.

On PIC18 that check cannot fire. Every PIC18 device declares its whole RAM
as one region (`p18f4550` is `[[0x0010, 0x07FF]]`), so `region_for` returns
the same region for any two addresses and `routine_base` always returned its
argument. Meanwhile `isel-pic18`'s `operand()` banks on the 256-byte `BSR`
boundary (`addr >> 8`). A routine frame straddling `0x100` therefore had its
`MOVLB` emitted in the middle of its own recipe.

Audited against the emitted asm (all 33 routines, every base `0x010`-`0x7E0`
on the p18f4550):

| recipe family | emits `MOVLB` inside a skip window? |
|---|---|
| unsigned mul / div / rem (all widths) | no: their loops end in real `BNC`/`BRA` |
| variable shifts (all widths) | no |
| `__sdiv_i8` / `__srem_i8` | no: no Z-chain carry fold at that width |
| `__sdiv_i16` / `__srem_i16`, `__sdiv_i32` / `__srem_i32` | yes |
| `__add_f32` / `__sub_f32` / `__mul_f32` / `__div_f32` | yes |
| `__cmp_f32` | yes |
| `__uitofp_f32` / `__sitofp_f32` / `__fptoui_f32` / `__fptosi_f32` | yes |

13 of the 33 routines. The first hazard site is typically
`BTFSC <frame byte>,7` immediately above a `MOVLB`, where the skipped
instruction names a frame byte in the other bank. Every hazarding base
crosses a `0x100` boundary; no base that stays inside one 256-byte bank
hazards, for any routine. A sim run of a source whose overlay puts
`__add_f32` astride `0x100` reads `10.0f` where `14.5f` is due.

The sibling question, whether the access bank's `0x5F/0x60` edge (the other
discontinuity `operand()` has) is also a hazard: audited, and it is not. A
frame crossing only that edge keeps one `BSR` value for its whole span, so
no `MOVLB` is emitted inside it at all.

## Decision

`routine_base` measures the bank as the core's actual operand granularity:

- **PIC18:** the 256-byte `BSR` bank, `(base >> 8)`, capped at the region's
  end so a partially-mapped bank does not claim bytes past it. A frame that
  would cross rounds up to the next `0x100` boundary, and `region_for`
  skips any unmapped gap between regions. The snap target is that boundary,
  never the region's start: a PIC18 `ram_banks` region spans many banks, so
  "the next region's start" is the address the frame already had.
- **PIC14/PIC14E:** unchanged, `region_for`'s region. Their banks are the
  hardware banks and they are distinct regions, so the two agree.

`round_if_routine`'s doc and the module doc lose the claim this work
disproved: that the float recipes' skips all target the next instruction.
The `PicBaseline` early return is untouched (its 16-byte banks cannot hold a
19-20 byte frame whole, and its recipes place each value single-bank
instead).

## Consequences

- A PIC18 program whose overlay crosses `0x100` now compiles correctly where
  it previously miscompiled silently. No program in the tree does today (all
  91 PIC18 fixtures are clean), so no recorded address moves; the fix is
  latent-defect closure plus a red-green regression test.
- Routine frames on PIC18 may start up to 255 bytes later than before. The
  frames are siblings and pack contiguously, so at most one frame per
  routine is displaced, and only when a boundary actually falls inside it.
- The `ram_banks`/`operand()` mismatch the issue called "only confusing" now
  has one owner: `routine_base` reads both and reconciles them in one place.

## Rejected alternatives

- **Enforce the invariant in `isel-pic18` by panicking on a straddling
  frame.** Turns a miscompile into a compile failure, but rejects programs
  that have a correct layout available; the allocator can just choose it.
- **Reserve the access-bank window (`docs/36`'s proposal).** Dead on
  arrival: the recipes no longer need `a=0` operands (ADR-031), so the only
  surviving constraint is the one-bank invariant, and a window reservation
  is a second mechanism for what rounding already provides.
- **Make the recipes branch-based so no bank constraint exists.** A
  rewrite of the machine-verified float bodies for a constraint that one
  rounding rule satisfies; ADR-015 already rejected the same trade.
