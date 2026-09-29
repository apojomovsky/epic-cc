# 47: RAM overlay audit findings (epic-cc#728)

Audit, not a production commitment: decompose our own RAM footprint on
`hal-pic18-menu-demo-18f4550` (the largest ladder row) into overlay,
globals, and fixed reserves, attribute each bucket to a mechanism in
`crates/alloc`, and turn the result into sequenced follow-up tickets.
Every count below is ours, measured from the driver's `--map` output
and `--emit asm` listing on current master. The comparison against a
reference compiler that motivated the audit lives in the private
epic-benchmarks repo (ADR-006); no reference numbers are quoted here.

## Our footprint (menu-demo, 18F4550)

Total 755 bytes: bank0 high-water 739 plus 16 fixed (retval/flag 4 with
a 12-byte ISR save area, `crates/driver/src/report.rs:fixed_bytes`).
The single GPR bank (`0x010-0x7FF`) splits into a frames-first overlay
`0x010-0x0E0` (about 208 bytes including a 14-byte disjoint ISR region)
with 44 globals stacked above it (`0x0E0-0x2D9`, about 515 bytes).
1342 locals across 128 functions share the overlay; 30 consts stay in
flash while 3 small const-to-RAM copies sit at the top of RAM.
Frames color well (menu_demo_init packs 192 values into 29 slots), so
the footprint is policy and lifetimes, not a broken coloring.

## Attribution

**Coloring floor.** Byval/sret params, allocas, and va regions take
full-function intervals; GEP bases and terms extend to the last use of
the GEP result; loop live-in/out stretches to block starts and
predecessor ends; phi destinations start at the earliest predecessor
end (`frame_layout`). Micro benches with almost no globals still show
the same small per-row excess, so this floor is coloring
conservatism, not overlay policy.

**Co-live copy pairs.** The docs/45 classes keep source and
destination live simultaneously: caller value plus callee byval across
the call, adjacent lanes that could merge. Each pair widens its
frame's peak (`locals_size`) and, through `base(f) = max caller
physical end`, every transitive callee base and the globals
high-water above them. Phi-edge coalescing already landed (#741)
with the isel self-copy skip.

**Honest structure, small.** Seven legalize-duplicated `_isr` frames
plus the dispatch body, the 14-byte region, the 12-byte fixed save.
Single-BSR-bank routine rounding (`routine_base`) costs about nothing
at current bases and becomes a tax only if the overlay grows past a
`0x100` boundary. Globals are mostly genuine C demand (task table,
TX/RX buffers, peripheral storage); sequential plus largest-first
bin-pack already closed the known bank-tail failure.

## Recommendation (landed as tickets)

- #754: const-to-RAM audit. Prove each copied const is read through
  a generic pointer path or move it back to the flash table path.
  Small globals win, no ABI change. First.
- #739 (exists): overlap-checked block-copy range merges, the
  same-frame half of the coalescing program. First, alongside #754.
- #737 (exists, left separate): call-aware byval slot reuse. Kept
  out because caller and callee frames are disjoint while co-live,
  so sharing needs a call-ABI design (caller-owned outgoing area or
  pass-by-address), not a coloring tweak. Sequenced after #754 and
  #739 via `blockedBy`. Largest prize, highest risk.
- #755: liveness-aware overlay pricing (init-once frames,
  single-call-site sharing, routine-rounding audit).
  Investigation first.
- #738 (exists, left separate): compute call results into the retval
  region. Isel-side flash work, about RAM-neutral.

## Residuals

- The `--map` dump used here was scratch and is deleted;
  reproduction is the driver command behind the ladder's menu-demo
  row with `--map` added.
- Routine-rounding and PIC14/PIC14E overlays are unpriced; their
  region shapes differ, so none of the above transfers without
  remeasuring.
- The startup-frame share (`__start`, `__epic_config`) was not
  split out; it sits inside the overlay depth above.
