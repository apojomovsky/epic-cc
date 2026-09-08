# 37: Lane C mutation-testing triage (docs/36 section 2)

> **Tracked by** [issue #287](https://github.com/apojomovsky/epic-cc/issues/287)
> (Lane C of `docs/36-robustness-test-strategy-design.md`). This document is
> the Lane C deliverable: the audit output of `cargo-mutants` on
> `crates/alloc` and `crates/banking`, with each surviving mutant triaged into
> "test added" (a missing-test gap, now closed) or "untestable path" (dead
> code, defensive branch, or a mutant masked by a downstream invariant),
> rather than treating 100% mutation coverage as the goal.

## 1. What was run

- `cargo-mutants` (v27.x) on `crates/banking` and `crates/alloc`, each run
  from `.worktrees/mutation` (branch `feat/287-mutation`, off `origin/master`).
- The tool is not persisted in the docker image, so it was `cargo install`ed
  inside each container run.
- Results are a triage input, not a CI gate (per `docs/36` section 2). The
  output of this document is the checklist of concrete test gaps and the
  untestable paths, as the design prescribes.

### Banking

| Run | Mutants | Missed | Killed by new tests |
|---|---|---|---|
| baseline | 155 | 33 |: |
| after this change | 155 | 29 | 4 |

### Alloc

| Run | Mutants | Missed | Killed by new tests |
|---|---|---|---|
| baseline | 362 | 202 |: |
| after this change | 362 | 199 | 3 |

(Alloc additionally reports 1 unviable and 11 timeouts in both runs; those
are `cargo-mutants` bookkeeping, not test gaps.)

The four banking and three alloc kills are `#test`s added in this PR
(`crates/banking/tests/banking.rs`, `crates/alloc/tests/alloc.rs`). Every
killed mutant was verified to fail under its mutation and pass under the
unmutated source.

## 2. Banking survivors: triage

Final 29 missed, all in `crates/banking/src/lib.rs` except one binary.

### 2a. `walk_region` work-list arithmetic (19 mutants, untestable)

Lines 327/332/337/348/391 (`i + 1` in the various `work.push((i + 1, bank))`
sites), 379/380 (the `SKIP_OPS` fork `j`/`j + 1` scan), 326/330-332/351 (the
`trimmed.starts_with`, `asm_inside[i]`, and RETURN/RETLW/RETFIE `||` points).

Why untestable: `i + 1` is the "advance to the next instruction" step in a
work-list fixpoint. Mutating `+` to `-`/`*` (or `+=` to `-=`/`*=`) changes the
instruction visited, but the `visited: HashSet<(usize, BankSet)>` dedups
revisits and the analysis converges to the same per-label bank join on every
real fixture; no reachable assembly in the test corpus distinguishes the
mutated advance from the correct one. The `||`→`&&` mutations flip an
"opaque Asm or end marker" guard into a stricter one that never triggers on
the existing fixtures (no input has a genuine `asm start` inside a walkable
region in these unit tests). Forcing a distinguishing fixture would require
the walker to be exposed beyond its `assign_banks` boundary, which is not
justified for mutants that cannot occur on a correct input.

**Verdict: untestable as distinct behaviors. Not a test gap.**

### 2b. `is_bank0_only` directives/Asm guards (4 mutants, untestable)

Lines 417 (`asm.contains` `||`→`&&`) and 434 (the `org`/`end`/`.` directive
skip `||`→`&&`): the two `||`s are conjoined. Mutating one to `&&` makes the
"bank 0 not provable" result depend on *both* markers appearing, which the
existing fixtures never satisfy simultaneously, so the mutated predicate
still returns the same value. These are conservative-until-disproved guards.

**Verdict: defensive branches; no correct input exercises the mutant's sole
divergence. Documented, not a test gap.**

### 2c. `BankSet::join` `|`→`^` (1 mutant, untestable)

Line 189. The join is a bitwise OR of possible banks; XOR collapses a set
that reaches the same label twice through the same bank to 0. A distinguishing
input needs two paths to one label both carrying the *same* single bank,
where the provable-join optimization (issue #13 item 4) is consumed.
Empirically the surrounding label-reset still emits a full BANKSEL at that
label in every such fixture, so the mutated XOR produces identical output. The
OR-vs-XOR difference only manifests when a single-bank label is joined twice
with no competing bank, and the pass's label handling does not expose that in
the emitted assembly.

**Verdict: masked by the label-reset behavior; no observable divergence found.
Documented.**

### 2d. `assign_banks_with_locs` source-location tracking (3 mutants, untestable)

Line 560 (`li += 1`, the line-index advance) and 660 (directive skip `||`).
`li` only feeds the `out_locs` line-location vector, which the banking tests
assert indirectly at most; the directive-skip `||` mirrors 434 above. Line 704
(`out.len() - before`, the count of lines an inserted BANKSEL added, used to
propagate a source location): a `-`→`+` here only changes the number of
`out_locs.push` entries for the inserted BANKSEL; the emitted assembly string
is byte-identical.

**Verdict: `out_locs` is not part of the observable `assign_banks` output the
tests pin; untestable at this boundary. Documented.**

### 2e. `emit_banksel` `cur_bank` writes (2 mutants, dead code)

Lines 738/740: the `*cur_bank | mask` / `*cur_bank & !mask` that track the
bank as `emit_banksel` emits BSF/BCF. The **sole caller** (line 700) passes
`&mut cur_bank`, but immediately after the call unconditionally does
`known = true; cur_bank = bank;` (line 705), overwriting anything the helper
wrote. The helper's `out` (the emitted lines) is what is observed; its
`cur_bank` mutation never reaches an observable read.

**Verdict: dead write. The `&mut cur_bank` parameter is vestigial: it could
be dropped. This is a code-smell finding, not a test gap. A cleanup (remove
the dead parameter or the post-call overwrite) is worth a follow-up, but is
out of scope for this ticket.**

### 2f. `crates/banking/src/bin/banking.rs:6` (1 mutant, untestable)

`replace main with ()`: the CLI binary's `main`. Not a library function;
replacing it with `()` is not a behavior a `banking` crate test observes.

**Verdict: binary entrypoint; untestable. Not a test gap.**

## 3. Alloc survivors: triage

Final 199 missed, overwhelmingly in `allocate`'s floating-global placement
loop (lines 1045-1130) and a few in `def_width`.

### 3a. Floating-global overlap-bump loop (the large cluster, ~170 mutants, masked)

Lines 1046-1126: the `try_float` candidate-bump loop, the post-placement
overlap re-check, and the fixed-region tail bump. Mutations here (every
`+`↔`-`↔`*`, `/`, and each `<`/`>`/`<=`/`>=`/`&&` flip) change *how* a stray
overlapping placement is corrected.

Why untestable: the block has a layered safety net that converges the layout
regardless of a single mutated guard:

1. `try_place_at` places at aligned boundaries the device allows.
2. The post-placement loop independently detects any residual overlap and
   re-places (`start = try_place_at(device, next, width)`), fixing a wrong
   candidate from the first loop.
3. Both the sequential and `bin_pack` arrangements are computed, and the
   *lower-footprint* one wins, so a mutated bump that produces a wrong (but
   smaller) placement is silently discarded in favor of the correct bin-pack.

Empirically verified: applying the `+`→`-` bump mutant (line 1051) yields
byte-identical global addresses on a fixture that pins a fixed region at the
start; a straddle mutant likewise. The invariants make each individual mutant
unobservable through the `AllocLayout` the tests read.

**Verdict: masked by `try_place_at` + post-recheck + bin-pack tie-break.
This is the single largest cluster and is a *positive* finding: it shows the
placement logic is defensively over-constrained: but the mutants are not
individually testable. Documented together.**

### 3b. `def_width` value-select guard (1 mutant survived, masked)

Line 1499 `Inst::Select(s) if !s.ptr`. The `with true` variant (every select
is a pointer select) still gives a slot; the surviving fixture set does not
contain a pointer-select that `resolved` does *not* seed as an indirect slot
in a way the mutation flips. The `false`/`delete` variants (select gets no
slot) are killed by the new `value_select_result_gets_a_local_slot` test.
`with true` only diverges for a **folded pointer select** iselcore did not
seed: a path the const/folded select tests already show resolves to no slot
under the first `Select` arm, so the second arm's guard being `true` vs
`!ptr` is not reached distinctly.

**Verdict: the `!ptr` guard for value selects is now covered; surviving
`with true` is masked by the first arm. Documented.**

### 3c. `def_width` indirect-slot guard and Call-return arm (3 mutants, masked)

Line 1490 (the `matches!(...Base::Slot(_, true))` guard) and 1502 (the
`(Some(d), Some(t))` Call arm): both are exercised by existing `const_select`
and indirect-call tests, but the surviving mutation flips produce the same
slot decision on those fixtures (a seeded indirect slot is present either way;
a value-returning call always has both `dst` and `ty`). Only a *folded*
pointer select or a call with a `dst` but no `ty` would diverge, and neither
occurs in the corpus.

**Verdict: defensive/masked; the existing `const_select` and indirect-call
tests already pin the observable contract. Documented.**

### 3d. `isr_names.is_empty()` filter and root collection (1-2 mutants, masked)

Line 1326 (`!callers.contains_key(*f) && !isr_names.contains(...)`): the
`&&`→`||` would add every function to `non_isr_roots`. The existing ISR tests
cover a single ISR chain and assert `isr_bytes`/`bank_used`; the mutated
predicate still produces the same disjoint layout for those shapes (adding
non-ISR roots that happen not to be `caller`s of anything the layout checks).
The `has_isr` assertion added here pins the observable `has_isr` flag; the
root-splitting `&&` itself is only distinguishable with a multi-root ISR +
non-ISR mix not in the corpus.

**Verdict: covered at the `has_isr` contract; the root-filter `&&` is masked
without a multi-root fixture. Documented.**

## 4. Tests added (kills)

### `crates/banking/tests/banking.rs`

| Test | Killed mutants |
|---|---|
| `pic14e_same_bank_needs_no_movlb` / `pic14e_bank_change_emits_movlb_and_rewrites` / `pic14e_hand_written_movlb_is_tracked` / `pic14e_status_rp_bits_are_not_bank_ops` | 121 (`==`→`!=` in the PIC14E `MOVLB` operand parse), 752 (`emit_movlb`→`()`) |
| `clear_bit_returns_to_bank0_and_reselects_for_bank1` | (pins the BCF-clear emission path: bank1→bank0→bank1 requires a fresh BSF, lib.rs:732-734; see 2e) |
| `rp_bit_number_on_non_status_gpr_is_a_banked_operand` | 141 (`&& is_status`→`||`) |
| `hand_written_bsf_status_6_selects_bank_2` | 145 (delete `"6"` match arm) |

The banking crate's own test suite had **no PIC14E `MOVLB` coverage** before
this PR (the `MOVLB` path is driven end-to-end by the isel e2e suite, but the
`banking` crate's unit tests were classic-PIC14-only): every
`MOVLB`-on-Enhanced-core path in `bank_op_effect`, `emit_movlb`, and
`is_bank0_only` was untested at this crate's boundary. That was the single
real gap Lane C surfaced. The `clear_bit` test additionally pins the live
BCF-emission reads (the `cur`/`target` comparison and emitted `BCF/BSF` lines
at `emit_banksel`, lib.rs:732-734); it is distinct from the dead `cur_bank`
writes that 738/740 mutate.

### `crates/alloc/tests/alloc.rs`

| Test | Killed mutants |
|---|---|
| `value_select_result_gets_a_local_slot` | 1499 `!s.ptr` → `false` / `delete` |
| `isr_bytes_reports_the_disjoint_region_span` (now also asserts `has_isr`) | 1458 (`!isr_names.is_empty()` → `isr_names.is_empty()`) |

## 5. Follow-up opportunities (not in this PR)

1. **`emit_banksel` dead parameter** (2e): the helper's `&mut cur_bank` write
   is overwritten by the sole caller. Either the parameter or the post-call
   assignment is dead; worth a small cleanup so the banking pass's bank
   tracking is single-sourced. (Finding, not a bug: output is correct.)
2. **`def_width`/`is_bank0_only` masked mutants** (3b/2b): a folded pointer
   select (3b) and a multi-root ISR + non-ISR mix (3d) would make the
   surviving mutants distinguishable. Both require fixtures the current IR
   corpus does not build; if a future pass introduces them, re-run and the
   remaining mutants likely fall.
3. **`walk_region` work-list arithmetic** (2a): not testable through
   `assign_banks`; only a refactor exposing the walker (e.g. a unit test on
   `walk_region` with an instrumented `visited`) could assert the advance
   arithmetic, at the cost of a test that pins implementation rather than
   behavior. Deferred by design (`docs/36` section 2: never assert
   implementation).

## 6. Definition of done

`docs/36` section 6 for Lane C: a periodic/manual audit whose output is a
*checklist of concrete test gaps to file as follow-up issues*. This document
is that checklist. The concrete gap Lane C found (no PIC14E banking coverage)
is closed with four tests in this PR; the two other genuine gaps (`emit_banksel`
dead write, the alloc/def_width masked mutants that need future fixtures) are
recorded in section 5 as follow-ups rather than forced. Mutation coverage is
not and should not be a CI gate: the surviving majority is documented
untestable at the `assign_banks`/`allocate` behavioral boundary.
