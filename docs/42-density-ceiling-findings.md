# 42: Density-ceiling findings (epic-cc#531)

Spike, not a production commitment: for each structural flash sink on
`hal-pic18-menu-demo-18f4550`, compute the best achievable number
without implementing the backend change, and turn the result into a
green/kill/reshape verdict on the integration ticket that owns it.
Method per sink is offline analysis of the emitted listing (or a
hand-lowered scratch copy), verified through the `Pic18` sim, gpasm,
and the existing oracle stack, the same trust argument `docs/41`
makes for the superoptimizer spike.

Baseline (current master): the `--emit asm` listing profiles at 12099
words and assembles to exactly 12214 flash words, matching
`fixtures/size_baseline.toml`. XC8 builds the same firmware in 9068
words. All counts below are against that baseline.

## Experiment 1: bank tracking (informs #495)

A BSR dataflow over the listing (meet-over-paths, joins by agreement,
calls clobber, PCL writes reset) finds 261 of 495 `MOVLB`s provably
redundant under current call rules: every path already holds the
selected bank. Pruning exactly those 261 lines drops the assembled
output from 12214 to 11927 words (see oracle note below on what
`reassembles` means here). The extra 26 come from branch
expansions the shrinkage avoids, a measured second-order effect, not
an estimate. Verdict: GREEN for isel-level join tracking, KILL for
the convention half of #495 as drawn.
Soundness rests on BSR being ISR-transparent (the prologue saves 0xFE0
to 0x00A and restores it; the ISR body holds no `MOVLB`), confirmed by
listing inspection.
gpasm is not a valid whole-listing oracle here: the unmodified listing
already fails it 37 ways (`.pclalign` directives, out-of-range short
branches our expansion pass absorbs); the pruned listing fails the
same 37. The round trip through `assemble_pic18` is the check that
carries weight, plus hand-traceable micro-listings for the analysis
itself.
Under a restore-on-return convention (best R=0), 323 `MOVLB`s go but
63 return sites each pay a restore word: net 260, indistinguishable
from join tracking alone (see above for the verdict and soundness notes).

## Experiment 2: W tracking (informs #502)

Textual-adjacency scan with liveness, bank-match, and SFR screening
(spike script, micro-listing selftest passing) finds 20 sites, 35
words: 15 `MOVWF`/`MOVFF` pairs (30 words) and 5 dead reloads (5
words), across 13 functions. Replaying #502's own profiler rule fires
108 raw pairs; validity filtering keeps 20, control flow blocking 79
of the 88 exclusions. Absolute upper bound with every raw pair dead:
187 words, plus about 42 words of near-miss gap pairs. Verdict:
RESHAPE, #502's 170-330 words do not reproduce as local tracking on
current master (codegen drift since the estimate). The near-miss
pairs are the larger remaining shape and want their own scoping.

## Experiment 3: const-init tables (informs #504)

A MOVLW/MOVWF pair is 2 words per byte; a table plus TBLRD loop costs
span/2 plus a fixed loop the #486 loop shape prices at 9 words, and a
TBLPTR-seeded variant at about 14. Current master carries only 607
const-materialization words across 424 sites (1.4 words per site):
hundreds of isolated 1-2 byte stores no fixed-cost loop can win.
Profitable spans total 20 words at a 10-word loop, 7 at a
14-word loop, 47 even at an aggressive 6. Verdict: RESHAPE, #504 as
scoped (300-500 words) does not reproduce on current master. The
remaining sink is tiny-site density, which needs a different lowering;
symbol-ref spans (58-74 words) additionally need table relocations.

## Experiment 4: placement (informs #493 and alloc follow-ups)

The access window (0x010-0x05F, 80 placeable bytes) holds 79 bytes of
overlaid frames by design (#512): no headroom for hot-slot placement,
and the 106 cover bytes behind the 234 surviving `MOVLB`s would not
fit anyway. The hottest frame bytes are the lever instead: the top 80
carry 86.9% of resolvable banked accesses. Verdict: GREEN for a new
hotness-sorted frame-layout ticket (a RAM-neutral permutation of the
same 80 window bytes, so the no-RAM-growth constraint holds by
construction), exact word saving to be measured by implementation.

## Recommendation

Land join tracking first (#534, about 260 words, no RAM cost, no
convention change), then re-profile: it composes with the layout work
(#535) and shrinks every other sink's denominator. #504 should be
reshaped or closed before anyone implements it as written; #502 keeps
only its near-miss shape; #495's convention half is killed by #534.
Both new tickets carry `blockedBy` #531 and `epic:density`.
