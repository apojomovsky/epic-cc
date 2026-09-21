# 42: Density-ceiling findings (epic-cc#531)

Spike, not a production commitment: for each structural flash sink on
`hal-pic18-menu-demo-18f4550`, compute the best achievable number
without implementing the backend change, and turn the result into a
green/kill/reshape verdict on the integration ticket that owns it.
Method per sink is offline analysis of the emitted listing (or a
priced hand-lowering construction), checked by reassembly through
`assemble_pic18`, hand-traceable micro-listings, and gpasm attention
where it applies. The mdb UART gate was not run on any scratch
artifact (see Residuals): behavior equivalence rests on the no-op
deletion arguments plus reassembly, not on execution. Same trust
shape as `docs/41`, narrower oracle.

Baseline (current master): the `--emit asm` listing profiles at 12099
words and assembles to exactly 12214 flash words, matching
`fixtures/size_baseline.toml`. XC8 builds the same firmware in 9068
words, so this listing sits at 1.35x; the 1.5-1.76x framing elsewhere
covers the real-target variant (13820 words) and tree drift between
measurements. All counts below are against the 12214 baseline.

## Experiment 1: bank tracking (informs #495)

A BSR dataflow over the listing (meet-over-paths, joins by agreement,
calls clobber, PCL/PCLATH/PCLATU writes reset, BTFSC/BTFSS/DECFSZ
modeled as two edges to next and next-after-next) finds 261 of 495
`MOVLB`s provably redundant under current call rules: every path
already holds the selected bank. The 495 denominator is a recount on
current master; #495's 947 predates the frames-below-globals landing
and subsequent growth. CPFSEQ/SLT/SGT do not occur in this listing.
Soundness rests on BSR being ISR-transparent (the prologue saves 0xFE0
to 0x00A and restores it; the ISR body holds no `MOVLB`), confirmed by
listing inspection. That invariant is version-sensitive: #534 must
assert it (fail if the ISR region ever contains a `MOVLB`), recorded
as a comment on #534, not assumed silently.
gpasm attention is limited: the unmodified listing already fails it 37
ways (`.pclalign` directives, out-of-range short branches our
expansion pass absorbs), and the pruned listing fails 37 by count
with the same failure classes. Line-level identity across the address
shift is explicitly not established and not load-bearing: the
`assemble_pic18` round trip (12214 to 11927, exact) plus the
micro-listings carry the verification.
Under a restore-on-return convention (best R=0), 323 `MOVLB`s go but
63 return sites each pay a restore word: net 260 against join
tracking's measured 287. The restore side's own interaction term is
unmeasured, so the honest reading is within about 10 percent, and the
kill stands regardless: the convention's fixed 63-word tax and its
complexity buy nothing join tracking does not already get for free.

## Experiment 2: W tracking (informs #502)

Textual-adjacency scan with liveness, bank-match, and SFR screening
(spike script, micro-listing selftest passing) finds 20 sites, 35
words: 15 pair rewrites (30 words) and 5 dead reloads (5 words).
Replaying #502's own profiler rule fires 108 raw pairs; validity
filtering keeps 20, control flow blocking 79 of the 88 exclusions.
Absolute upper bound with every raw pair dead:
187 words, from 79 shape-a pairs at 2 words each plus 29 shape-b at 1
each (158 + 29). Near-miss gap pairs add about 42 words: 27 pairs
whose matching reload sits 1-3 instructions later across only
W-preserving neutral instructions (observed gaps are MOVFF/CLRF, never
MOVLB), with no label, CALL, W-clobber, or f access between; same
per-site saving, liveness unproven, hence approximate. Verdict:

## Experiment 3: const-init tables (informs #504)
A MOVLW/MOVWF pair is 2 words per byte; a table plus TBLRD loop costs
span/2 plus a fixed loop. The loop is priced, not hand-lowered: body
identical to #486's sim-gated copy loop (MOVFF TABLAT to POSTINC,
DECFSZ, BRA: 5 words) with TBLPTR seeding in the observed
epic_harness_init idiom (LOW/HIGH/UPPER plus three MOVWF: 6 words),
LFSR and count (3 words), about 14 fixed in all. Current master
carries only 607 const-materialization words across 424 sites (1.4
words per site): hundreds of isolated 1-2 byte stores no 14-word
loop can win. #504's 1016 words and 410-word menu_demo_init site
predate the zero-init coalescing, shift-sequence, and GEP-scaling
landings that rewrote init codegen since; the largest pure span today
is 22 bytes for 25 words. Profitable spans total 20 words at a
10-word loop, 7 at 14, 47 even at an aggressive 6. Verdict: RESHAPE,
#504 as scoped (300-500 words) does not reproduce on current master.
The remaining sink is tiny-site density, which needs a different
lowering; symbol-ref spans (58-74 words) additionally need table
relocations, untested here.

## Experiment 4: placement (informs #493 and alloc follow-ups)

The access window (0x010-0x05F, 80 placeable bytes) holds 79 bytes of
overlaid frames by design (#512): no headroom for hot-slot placement,
and the 106 cover bytes behind the 234 surviving `MOVLB`s would not
fit anyway. The hottest frame bytes are the lever instead: the top 80
carry 509 of 586 resolvable banked accesses (86.9%). Resolvable here
means a bank-suffixed access whose absolute address the exp1 analysis
resolves: bank-known state, address below 0xF00 (SFRs excluded),
unknown-state accesses excluded; 752 bank-suffixed accesses total, 586
resolvable. Verdict: GREEN for a new hotness-sorted frame-layout
ticket (#535, carrying the 509 proxy in its acceptance): a
RAM-neutral permutation of the same 80 window bytes, so the
no-RAM-growth constraint holds by construction, exact word saving to
be measured by implementation.

## Residuals (what this spike does not establish)

- mdb UART gate: not run on any scratch artifact. The pruned listing
  is an offline deletion, not a driver build, so no hex exists to
  gate; equivalence rests on the no-op argument plus reassembly.
  #534 must earn the gate through its own e2e fixtures instead.
- Scratch scripts deleted: the four analyzers and the pruned listing
  were removed before the PR; only this doc ships. Reproduction:
  re-emit the listing with the driver command in the ticket
  (includes, defines, and inputs match the `hal-pic18-menu-demo`
  size case), rerun the profiler, and reimplement the scans from
  each experiment's stated rule. Exp1 additionally keeps its
  micro-listing contract in prose (agreeing join prunes,
  disagreeing join keeps, CALL clobbers under A and preserves
  under B).
- Hand-lowering substituted: charter asked for three lowered sites
  with sim runs; the prize (7 words at honest loop cost) did not
  justify them, so the loop is priced from #486's landed shape
  instead, stated above.

## Recommendation

Land join tracking first (#534, about 260 words, no RAM cost, no
convention change), then re-profile: it composes with the layout work
(#535) and shrinks every other sink's denominator. #504 should be
reshaped or closed before anyone implements it as written; #502 keeps
only its near-miss shape; #495's convention half is killed by #534.
Both new tickets carry `blockedBy` #531 and `epic:density`.
