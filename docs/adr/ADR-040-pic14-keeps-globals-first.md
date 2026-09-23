# ADR-040 -- PIC14 keeps globals-first: the dispatch bank-switch sink is not an alloc-placement problem

Status: Accepted 2026-09-23<br>
Decides: epic-cc#600

## Context

epic-cc#600 profiled `hal-pic16-encoder-full-16f877a` with
`scripts/density-profile.py`: 6909 flash words, 978 bank-switch words,
of which 272 sit in `epic_dispatch_all_irqs` (130) and its `_isr` twin
(142). A tracked-bank simulation over the emitted asm attributes the
main function's 130 as 52 SFR-to-frame flips (INTCON/PIR flag reads in
bank 0 against a bank-2 frame), 30 callback-global-to-frame flips
(`g_t0_overflow_cb` and kin in bank 1 against the same frame), and the
rest label-join full BANKSELs. The twin mirrors this with its frame in
bank 3.

The issue proposed beating the pair down by half or more through
placement (co-locate dispatch spills with flag banks), scheduling, or
bank tracking. The banking pass already tracks across the spill
sequences; the switches it emits are necessary given where the bytes
live. So the question put to `crates/alloc` was whether moving the
bytes closes the gap.

## Decision

PIC14 keeps the historical order: globals from the GPR start, frames
above them. No frames-first, no heat-partitioned globals, on any core
without an access bank. epic-cc#600 closes with this ADR as its
deliverable; whatever is left of the sink belongs to narrower tickets
(see below), not to a retry of placement.

## Rejected alternatives

All three were implemented, measured on the #600 fixture, and
reverted. Figures are listing words from the same profile run.

- **Spill-free indirect-call compare (isel).** Compare the fp value
  straight from its global instead of the frame spill. Rejected as
  unsound, not just slow: the spill is the IR load's single snapshot
  feeding both the null-check icmp and the candidate chain, and
  re-reading the global replaces it with N reads. A null-check passing
  on a stale value followed by a chain read of nulled bytes falls
  through every candidate into the trap loop. About 30 words of prize
  for a new hang shape.
- **PIC14 frames-first (alloc).** Overlay below the globals on every
  core, mirroring ADR-036. Main dispatch 130 to 97, twin 142 to 112,
  but program bank-switch 978 to 968 and total 6909 to 6984: the
  globals pushed into banks 2 and 3 cost every other function more
  than the dispatch saves, plus alignment padding growth.
- **Heat split (alloc).** ISR-context-referenced globals of 8 bytes or
  fewer ahead of the frames, frames next, cold globals last. Pair 272
  to 188, program total 978 to 1033. The structural limit: bank-0 GPR
  is 80 bytes and cannot hold both hot contexts. The main dispatch
  fits (frame 0x5A, callbacks 0x20 to 0x28, SFR flips gone) but the
  ISR region starts after the whole main context, so the twin lands in
  bank 1 and keeps flip-flopping every flag.

## Consequences

- `crates/alloc` order is unchanged; the deferred frame-base
  derivation and the heat scan from the two prototypes stay out. A
  future placement proposal must beat program totals, not just the
  pair, because both prototypes won the pair and lost the program.
- The census method (tracked-bank simulation attributing each switch
  to a bank pair) is reusable for the next density ticket on this
  fixture.
- Candidate follow-ups, each its own ticket with its own baseline:
  label-join bank analysis across indirect-call chains (about 60
  full-BANKSEL words per dispatch function today), an ISR-region-first
  variant (zeroes the twin, restores the main dispatch to about 130,
  marginal either way), and the larger oracle-gap slices on the same
  program (dead-store-reload 216, zero-init-pair 136).

## Revisit if

A target ships a bigger bank 0, the dispatch checks fewer flags per
pass, or a placement variant is measured beating both the pair and
the program total on this fixture. Pair-only wins do not qualify.
