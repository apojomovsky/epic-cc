# #675 BSR-at-calls pricing notes (WIP, delete before merge)

Method: `scripts/bsr-price-wip.py`, an instruction-level BSR dataflow
fixpoint over `scratch/size-report/menu-demo.asm` (fresh, worktree
master 1c55049: 201 MOVLBs). Model: MOVLB sets bank; CALL/RCALL takes
callee summary (unknown unless proven); labels join all preds
(including back-edges); MOVFF to BSR poisons. Validated: reset-on-label
+ clear-on-call linear sim reproduces the ticket's "exactly 1
redundant" on the old listing.

Fresh-listing buckets (fixpoint with calls unknown: 17 redundant;
with fixpoint exits: 24, delta 7):

1. Init sequence (`__start` RAM init, isel-pic18 `select()` line ~9558):
   6 MOVLBs, 4 redundant (banks 1,1,1,1,2,2, straight line, no
   label/call between). Cause: untracked explicit MOVLB per byte.
   Fix: track last bank across the init loop. Yield 4 words, zero
   soundness risk. CHEAPEST, implement.
2. Forward join misses (isel-tracked, single unkilled agreement): 5
   (EPIC_IRQ_Restore L3/L10 single-pred, gpio4_delay_us L4
   single-pred, redraw switch-chain labels x2). Probable cause:
   `bsr_dirty` boolean (terminator lowering selected a bank on the
   shared linear prefix, end forced unknown). Fix: edge-vs-prefix
   precision. Yield ~5 + exit knock-ons. Moderate soundness risk.
   Deferred: price recorded, fix not attempted (lean quota).
3. Loop headers (back-edge agrees, isel keeps unknown deliberately):
   8 (epic_tick_delay_ms, redraw_L51, redraw_status x2, strlen,
   timer0 ISR, __udiv x2). Needs emission-order to fixpoint upgrade.
   Deferred.
4. Exit agreement (needs callee summary isel lacks): 7 total.
   - Ordinary callees, likely bsr_dirty-limited, fix (2) may unlock:
     EPIC_IRQ_DisableSrc, EPIC_EEPROM_ReadByte, epic_taskmgr_ticks.
   - Recipes (no Gen run, unknown by construction): __shl_u32,
     __mul_u8 x2. Needs recipe-body exit analysis/annotation.
   - ISR callee (forced unknown): epic_serial_on_tx_isr via indirect
     dispatch. Needs ISR exit-contract change.
5. Callee-saves-BSR: cost 4 words per function (MOVFF pair each way)
   plus a temp slot; benefit at most 1 word per call site and only
   when the pre-call bank matches. Menu-demo recipe call sites are
   single digits per function. Net negative. Recorded as stays.

Caveats: model runs post-outline/relaxation; outline RCALLs are
treated as unknown exits (pessimistic only: hides, never invents,
candidates). Computed jumps (ADDWF PCL switch tables) inherit the
jump-site bank along table entries, matching IR-level join coverage.
