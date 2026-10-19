# 1370: walk-aware temp homes for 32-byte copy loops (83B to 70B)

Ephemeral plan, deleted before merge. Ticket: epic-cc#1370. Base: origin/master
with #1372 in. Measured base: memcpy-32-16f877a 1281 cyc / 89 flash / 83 RAM.

## Where the 13 bytes are (verified against --map/--var-table/out.asm)

Bank0 0x20-0x6F (80) + common 3 = 83. Globals 67 (0x20-0x62). Frames:

- main 0x63-0x6A (8): counters %2/%10 i16 (0x63-0x64, shared slot),
  sum %11 + trunc %4 + fill-inc %5 (0x65), load %12 + fill-add %tmp +
  latch-cmps %6 + walk-inc %14 (0x67), sum-add %13 (0x69), latch-cmp %15 (0x6A),
  %5-high (0x66), %14-high (0x68). Unused call result %8 overlays counters.
- memcpy 0x6B-0x6F (5): counter %6 i16 (0x6B-0x6C), held %8 + dead inc
  %10 i16 (0x6D-0x6E), latch-cmp %11 (0x6F).
- fixed 0x70-0x72 (3): scratch (fused i16 latch claim), retval x2 (unused).

Asm never reads 0x64? No: 0x64 IS counter-high (read? `MOVF 0x63,W`
only reads low; high never touched, but slot stays). Never read:
0x66, 0x68, 0x6A, 0x6C, 0x6E, 0x6F, 0x70, 0x71, 0x72, 0x64(call part).

## Cut accounting (each verified against emission shapes)

1. Walk-dropped homes (iselcore walk match, single-block shape only):
   incregs (%5, %14, memcpy %10), latch-cmps (%6, %15, memcpy %11),
   dead chains (%tmp, %4 if free), dead truncs, dead and-links,
   walk-dead @-loads. Frees 0x66, 0x68, 0x6A, 0x6E, 0x6F (+0x67 with cut 3).
2. Sum coalescing (alloc edge add->phi, walk-gated): %13 into %11.
   Frees 0x69. Backedge self-copy elides via w_holds (verify).
3. Single-use walk loads in W: %12 (load, only use is the accumulate add,
   adjacent). New WalkOp::Accumulate (`MOVF INDF,W` then `ADDWF slot,F`).
   Frees 0x67 (with cut 1).
4. Unused call results: %8 takes no home; isel skips caller copy and
   callee Ret write; report counts retval by use. Frees 0x71-0x72 (+flash).
   0x64 stays (counter-high).
5. Scratch model: skip icmp_scratch for walk-fused latch compares
   (same iselcore match). Frees 0x70.

After cuts: main 3 (0x63-0x65), memcpy 3, fixed 0 = 73.
Dip (existing, no change): memcpy base = live_high = 0x63 (nothing live
across the call once %8 is homless), ends 0x66/0x69 both in bank0, so the
current bank-key check passes (the 1-byte refusal was an artifact of the
waste: old end 0x70). Union 0x63-0x65 = 3. Total 67+3+0 = 70.

## One-analysis architecture (ticket cut 1 requirement)

New iselcore module `walk`: `match_walks(m, f, ctx) -> Vec<WalkMatch>`
where ctx carries what alloc/isel/report each have identically:
IR + resolved (same fn) + folds' unplaced (each side's own; predicate
also declines on packed-touching memops, vacuous on PIC14) + prov
(same fn) + bounding/final globals (EQUAL per allocate_with_pool
exactness comment) + device windows as plain data (ram_banks,
common_ram) + const_fan_in for the pool proxy.

Subset rules (predicate declines; planner may still accept; never the
reverse): single-block shape only (multi-shape loops keep homes);
Base::Slot memop bases decline; unresolvable ptrs decline; region loads
in `unplaced` decline; store-src/value-base regs with const fan-in
decline (pool_slots proxy); latch/chain regs named only with full
consumption proofs replicated from walk_finish.

Consumers:
- alloc `unplaced` map (PIC14-gated, gated pass gets bounding globals):
  dead set per func. Shared planning+coloring via existing unplaced_in.
- isel find_walk_loops: single-block arm calls the same match, then
  materializes WalkLoop (count_slot/load slots via addrs; all present
  by construction; decline impossible). Load arm gains slot: Option;
  None emits `MOVF INDF,W` only. Accumulate op for cut 3.
- report fixed_uses: retval by use (iselcore dead-result helper,
  module-wide); icmp_scratch skip for match latch dsts.

Cut 2 edge in alloc beside phi coalesce with guards: width-equal,
single-block walk pair (add feeds carried phi, other operand is the
W-load or const), not retval_homed/unplaced, add's uses ⊆ {phi edge,
exit reads served post-copy}. Self-copies stay no-ops.

Cut 4 helper (iselcore): `dead_call_results(f) -> Set` (zero uses with
GEP propagation + Asm reads counting). alloc frame_layout skips defs;
isel emit_call/emit_indirect_call skip copies; isel Ret skips write
when module-wide no caller uses a result (direct sites decidable;
indirect/extern/address-taken keep); report counts call ty only if
dst used, f.ret only if some caller uses a result.

## Blast-radius gates

- Drops/coalesce/dead-results: Core::Pic14 only (alloc + isel + report).
  Other cores byte-identical.
- No baseline TOML moves (cc#1163 option 3): ladder fails only on
  exceed; name rows in PR body.
- Proof: ladder rows both kernels x4 profiles + new checksum pins
  (memcpy 0x30, memset 0xA0) in expected_results; full suite green.

## Order

1. iselcore walk match + tests (unit, std only).
2. alloc unplaced extension + bounding-globals threading.
3. isel materialize (Load None, Accumulate), dead-result skips.
4. alloc cut 2 edge + cut 4 frame skip; report cut 4 + cut 5.
5. Measure (map/asm/cycles), ladder pins, full suite.
