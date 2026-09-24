# XC8 size-parity microbench ladder (design)

Date: 2026-09-22. Scope: Approach A, approved in brainstorming.
Goal: match XC8 v4.00 `-O2` flash size on PIC18, measured per operation,
with the menu demo kept as the headline gate.

## Status

The superopt track scoped in `docs/41-superopt-spike-findings.md` is
landed or closed with a recorded negative: bool materialization
(superseded by the preclear lowering), left and right shifts by 4 and
the 16-bit amount 4-7 family, 32-bit amounts 6-7, amounts 2-3 proven
minimal at the searched bound, 32-bit amount 4-5 verified and losing
(deliberately not landed), the amount-7 byte-move form proven unsound.
The current menu-demo baseline lives in
`crates/driver/tests/fixtures/size_baseline.toml`.
XC8 reference numbers live in the private epic-benchmarks repo
(benchmark results are licence-confidential, ADR-006).

## Non-goals

XC8 is a black-box size oracle only: invoke `xc8-cc`, read its size
report. No disassembly, no reverse engineering (ADR-006). No XC8
correctness differencing in this track. No SMT or LLM search. No PIC14
scope. No change to the menu-demo headline gate.

## Corpus

One small C bench per sink in `scripts/density-profile.py` `SINK_RULES`
precedence order, each sim-correct plus flash-word pinned:

1. shift-chain: shl/lshr at widths 8/16/32, amounts 1-7 and 12/28.
2. wide-const-materialization and wide-literal-arith: multi-byte
   materialization with zero and nonzero lanes.
3. zero-init-pair: zero stores vs CLRF.
4. struct-copy-movff: copy sizes around `COPY_LOOP_MIN_PAIRS`.
5. switch-compare-chain vs switch-jump-table: dense and sparse switches.
6. bool-materialization: compare to byte slot (regression pin only).
7. dead-store-reload: parked value read back (blocked on #502
   W-tracking, filed as prerequisite, not worked around).
8. bank-switch and sfr-context-save: placement counts.

Benches live with the maintained suite (same layout as the existing e2e
fixtures), so they are durable coverage, not scratch probes.

## Runner

Each bench compiles twice for `18F4550`: once with `epic-cc`, once with
`xc8-cc -mcpu=18f4550 -O2` through the `xc8-oracle` image wrapper
(`make oracle-image` once, `make oracle-exec`), which supplies `-mdfp`.
Flash words come from each toolchain's own size report. A compare step
ranks the per-bench gap in words and refreshes the XC8 snapshot row
deliberately, never per run.

## Landing rule

Profile, rank the XC8 gap, file one ticket per sink with `blockedBy`
where #502 is prerequisite. Each fix is superopt-verified where it
applies, then the real selector output is re-simulated over the full
domain before landing, with the dead-W precondition stated explicitly
until W-tracking exists. Growth in any bench or ladder entry fails the
gate; shrinking is free.

## Tickets out of this spec

Per-sink implementation tickets, the #502 prerequisite, the XC8
snapshot refresh helper, and the bench harness itself. Each goes on the
board with area labels before its PR.
