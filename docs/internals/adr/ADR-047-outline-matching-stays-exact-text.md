# ADR-047 -- Outline matching stays exact-text under frame renumbering

**Status:** Accepted 2026-10-08<br>
**Decides:** `epic-cc#882`<br>
**Evidence:** pick-dump analysis of bridge and menu listings with and
without outlining, base vs #853's prototype, old base and master

## Context

The outliner (`outline`, ADR-042) merges repeated runs by exact
normalized text, absolute RAM addresses included. #853's prototype
shrinks frames, and `base(f) = max caller end` renumbers every address
downstream. On the old base this broke merges badly: bridge outlining
recovered 1189 words instead of 1343, turning a 100-word no-outline win
into +54 flash with outlining on. Every remaining RAM lever threatened
the same flash tax, so #882 asked for matching that survives
renumbering.

## Measurements (ours only, 2026-10-08)

Rebased #853's prototype onto master in scratch and measured the full
30-row `-Os` ladder base vs stacked: bridge 13575 to 13561, control
9519 to 9510, blink unchanged flash with RAM 30 to 26, all other rows
identical, except menu 9311 to 9313 (+2). The +54 crisis does not
reproduce on master; only menu grows.

Menu +2 is banking churn, not merge breakage: no-outline menu grows +51
(10735 to 10786) with MOVLB count 254 to 257 (the heat-permutation
resort #853 diagnosed), while outline recovery on menu improves
slightly. A shape change (added bank selects), not a lost address
coincidence, so no matching tweak recovers it. It stays with #853.

Recovery is 83% intra-function on bridge (86% on menu); the rest is
cross-function runs coinciding on overlaid addresses.

## Decision

No change to matching. Exact-text matching is optimal for identical
sharing: two sites can share one static body only when every operand is
equal, so any matcher that still demands operand equality finds the same
groups exact text finds. Cross-frame coincidences are inherently
layout-sensitive; no equality-demanding matcher keeps them across a
re-tiling.

## Rejected alternatives

- **Shape-abstracted matching with an identical-operand filter:** the
  same partitions as exact matching by construction. No-op.
- **Outlining on symbolic slots before address assignment:** forfeits
  all cross-function merges, 14 to 17% of recovery (about 200 words
  each on menu and bridge). Rows would grow.
- **Stable frame bases:** needs slack or sticky placement, a RAM cost
  that inverts the RAM epic, and alloc-side work outside this ticket.
- **Parameterized bodies:** an FSR setup (2-word `LFSR`) matches or
  exceeds the small idioms that dominate recovery. Uneconomical.
- **Numeric-spelling canonicalization:** zero new merges on bridge and
  menu (131 spelling variants live in non-repeated contexts, `equ`
  names never appear as operands). No-op on real listings.

## Consequences

- #853 is unblocked: its stacked prototype shows no outline-driven
  flash growth on master. Its menu +2 is heat/bank churn for its own
  rebase to own.
- Future RAM levers should still measure the outline on/off split when
  they churn addresses: regrouping is real (79 regrouped runs on
  bridge) even though it nets small on master today. A lever whose
  no-outline delta and outlined delta disagree by more than noise owns
  the difference.

## Acceptance accounting

Against #882's criteria: no code changed, so no ladder row moves under
this ticket and the e2e sim tests plus the fuzz gate stay green (full
suite green 2026-10-08). The stacked-#853 proof leaves one exception:
menu grows 9311 to 9313 (+2), owned by #853 for its rebase (heat/bank
churn, shown above to be outside merge matching). Merging this record
accepts that exception as #853's scope, not a waiver of the no-growth
ratchet for future work.
