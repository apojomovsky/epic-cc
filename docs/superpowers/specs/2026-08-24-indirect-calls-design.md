# Indirect calls through function pointers -- Design

**Status:** draft (pending approval)
**Date:** 2026-08-24
**Parent:** `docs/03-decisions.md` ADR-002 (whole-program compilation), ADR-003 (static allocation)
**Ticket:** `epic-cc#73`
**Scope:** `epic-cc` compiler only; the HAL guard removal is tracked in `apojomovsky/epic-hal#67` (contract section 12).

---

## 1. Goal and non-goals

**Goal:** a C program may call through a function pointer whose value is selected
at runtime, on both `p16f877a` (PIC14) and `p18f4550` (PIC18), with call-depth and
recursion checks that account for the indirect edges. This is the compiler-side
half of the HAL's callback API (`epic-hal#67` compiles every peripheral callback
dispatch out today).

**Non-goals (v1):**

- Runtime address-to-function `inttoptr` (`#117`): a function pointer built from a
  computed literal has no statically known target set and panics loudly (unchanged).
- `const` flash tables of function pointers (`static const` dispatch tables): the
  global-initializer decode currently panics on `ptr @f` elements and keeps panicking.
  The HAL registers callbacks into RAM structs (`{cb; state}`), which is the shape v1
  targets. A `const` table is a natural follow-up, not part of #73's acceptance.
- `ptrtoint` on function addresses (comparison of fp values beyond `== null`,
  arithmetic on them): unsupported, loud panic (unchanged).
- PIC14 computed-PCL dispatch tables: a compare-and-call chain is chosen for BOTH
  cores (see §4); a PCLATH-computed jump is a future code-size optimization, not v1.

---

## 2. Empirical ground truth (clang 20.1.8, `-target msp430 -O1`, as pinned)

All shapes below were captured from probe fixtures this session.

**Shape A -- select-then-call (the common case, e.g. `if (cond) fp = g; fp(x);`):**

```llvm
%2 = icmp eq i16 %1, 0
%3 = select i1 %2, ptr @add, ptr @mul
%4 = tail call i16 %3(i16 noundef 3, i16 noundef 4) #2, !callees !6
```

**Shape B -- struct member callback (the HAL's exact pattern):**

```llvm
store ptr @on_event, ptr @g_dev, align 2        ; main registers the callback
%3 = select i1 %2, ptr @on_event, ptr @on_other
store ptr %3, ptr @g_dev, align 2
tail call void %3(i8 noundef zeroext 42) #2
```

**Shape C -- function pointer parameter:**

```llvm
define void @register_cb(ptr noundef %0) {       ; a cb flows in as a value
  store ptr %0, ptr @g_dev, align 2
  ret void
}
```

**Shape D -- dispatch table (`fp = tbl[idx]; fp();`): emits `!callees` NOT AT ALL**:

```llvm
@tbl = dso_local constant [2 x ptr] [ptr @a, ptr @b], align 2
%5 = load ptr, ptr %4, align 2
%6 = tail call zeroext i8 %5() #2                ; no !callees metadata
```

`!callees` is an optimizer hint, not a correctness guarantee (D proves it is
absent for table loads). **The compiler must never dispatch on it alone.** The
sound candidate set is whole-program: every function whose address appears as a
value in the module.

LLVM text call syntax: the callee is the first `@` or `%` token after the return
type; the irparse callee-split at `lib.rs:2228` already finds both, and
`.trim_start_matches(|c| c == '@' || c == '%')` currently erases the distinction
(the bug this ticket fixes).

The canonical IR text (`crates/ir`), `irparse`, `wholeprog`, `legalize`, `callgraph`,
`alloc`, `isel`, `isel-pic18` all consume `Call.func: String`; callgraph and
wholeprog already skip numeric callees (the #70 partial fix).

---

## 3. Approaches considered

### A -- Compare-and-call chain over the address-taken set (chosen)

An indirect call site lowers to an inline chain over the module's address-taken
functions, in deterministic (sorted-name) order:

```
; PIC14 shape (per candidate f):
    MOVF <fp_slot>, W            ; low byte of the fp value
    XORLW LOW(f)                 ; assembler-resolved label literal
    BTFSS STATUS, 2
    GOTO L_next
    MOVF <fp_slot+1>, W
    XORLW HIGH(f)
    BTFSS STATUS, 2
    GOTO L_next
    ; matched: copy args into {f}::param slots, MOVLW PAGE(f); MOVWF PCLATH;
    ; CALL f; PCLATH restore; GOTO L_done
L_next:                          ; fall through to the next candidate
    ...
L_trap:  GOTO L_trap             ; no candidate matched: deterministic trap
L_done:
```

PIC18 uses the identical structure with `BNZ L_next` / `BZ L_match` short
branches (the compare is 4 instructions, so the branch range is trivially
satisfied) and ends arms with an absolute `GOTO L_done`.

*Pros:* correct on both cores; uses only existing instructions (both simulators
already model every one: MOVF/XORLW/BTFSS/GOTO/CALL on PIC14, MOVF/XORLW/BNZ/BZ/
CALL/GOTO on PIC18); no new runtime routine; no new assembler syntax (LOW/HIGH/
PAGE label resolution exists in the two-pass assembler); dispatch is
deterministic and page-safe (each arm does the direct-call PCLATH dance); the
trap gives a loud, deterministic failure for a bogus fp.

*Cons:* O(#candidates) compares per call site. The address-taken set in a real
HAL program is a handful of callbacks; a 10-candidate site costs ~10 arms of
~10-16 words. This is exactly the "compare-and-call chain" shape the issue
calls out as the conservative option. An optimized path (computed-PCL dispatch
on PIC14) is a later ADR, not v1.

### B -- Computed-PCL dispatch table (PIC14 only)

A shared table trampoline: `W = LOW(fp)`, `MOVWF PCL` with PCLATH set from
`HIGH(fp)`; each candidate's entry `GOTO f`. The compiler already owns this
exact trick in the const-table readers (`__read_*`: `ADDLW LOW(table); MOVWF
PCL`).

*Rejected as the v1 mechanism:* the PIC18 simulator has no PCL-write model
(`write_f` treats `0xFF9` as a plain RAM byte; PCL reads exist at `0xFF9` for
`MOVFF` only) so PIC18 would need simulator surgery or a different lowering
anyway; and a computed-PCL jump needs each candidate entry to know the runtime
W value, forcing an extra register dance. Keeping ONE lowering across cores
wins on correctness risk. A PIC14-only table can follow as a size
optimization.

### C -- Runtime dispatch library routine (`__dispatch`)

A shared routine taking fp + candidates. *Rejected:* a shared routine's
arguments would need a fixed ABI slot arrangement inside the frame overlay
(the routine's frame must be overlaid against every indirect caller), and
the chain is only ~10 words inline per site. Inline wins.

---

## 4. Pipeline changes

### 4.1 `Call` gains `callees: Vec<String>`

- empty for a direct call (the existing `func: String` keeps the target name);
- non-empty for an indirect call: `func` holds the SSA register name (numeric,
  as the parser produces today) and `callees` is the sorted candidate list.

Canonical text (`ir::serialize` / `ir::parse`):

```
%4 = call i16 %3(i16 3, i16 4) callees add mul
```

The `callees` list round-trips; absent list = direct call. The LLVM-text reader
(`irparse`) sets `callees = []` and leaves the numeric `func`; the candidate
computation happens later (see 4.4), so the `.ll` boundary stays a pure parse.

### 4.2 `irparse`

- `call %3(...)`: keep the current numeric `func`, no longer strip the `%`
  distinction — the callee token's sigil decides direct vs indirect:
  `@name` → direct (current behavior); `%reg` → indirect (marker, empty
  `callees`). The `!callees` metadata is dropped with the trailing `, !...`
  cut (already true at `lib.rs:2180`) — never consumed.
- Global values stay as `Val::Global(f)` wherever they appear (select,
  store val, call arg, phi, icmp) — no change; they are the raw material for
  the address-taken set.
- Unchanged: `store ptr @f` into a RAM global parses fine today (it is a
  `Val::Global`); only the *codegen* of such a value needs the new literal
  path (4.6).

### 4.3 `wholeprog`

`check_calls_resolved` keeps skipping whole-number `func` (indirect calls have
no static target). No change.

### 4.4 `legalize` (the single decision site for candidates)

After all existing lowering and the interrupt duplication:

1. **Rewrite function-address values inside the ISR context.** After
   `duplicate_isr_shared` created `{name}_isr`, every value operand that
   references a *shared* function inside an ISR-context function is rewritten
   `@f` -> `@f_isr`, exactly like the existing call-target rewrite. Without
   this, the ISR's stored callback pointer would point at the main-context
   copy and the ISR would silently run in the main region's frames (a
   miscompile; the disjoint-ISR-region guarantee depends on it).
2. **Compute the address-taken set** A = { g : `Value::Global(g)` appears in
   any instruction of the post-legalize module }. (Non-const globals with
   `ptr` initializers are zeroinit and contribute nothing; const tables panic
   at parse — out of scope.)
3. **Compute the two contexts** once: ISR-ctx = transitive closure of the ISR
   roots over the *direct* call edges; main-ctx = everything else.
4. **Fill `callees` for every indirect call site**: a site inside an
   ISR-context function gets `A ∩ ISR-ctx`; every other site gets
   `A − ISR-ctx`. Sorting is deterministic (name order).

This split is load-bearing: the candidate list for an ISR site must not
include a main-context callback, and vice versa, or the overlay allocator's
disjoint-region analysis would see the ISR reaching a main-context frame (and
the main context reaching an ISR copy).

5. **Validate** every `callees` member exists (self-check, panic loudly).

### 4.5 `callgraph` and `alloc`

- `build()`: for each `Inst::Call` with non-empty `callees`, add ONE edge per
  candidate (`caller -> f` for f in callees) to both `edges` and the
  adjacency. The numeric-`func` skip stays as a belt-and-braces guard.
- `alloc`: no change; it consumes the edge text. Because the candidate sets are
  context-restricted, the ISR/main region split survives (an indirect caller
  only ever references functions of its own context).
- Depth check: conservative depth now includes the chain depth (each
  indirect edge adds 1). Some legal programs can now fail the 8-level check;
  that is the issue's explicitly accepted trade-off and is recorded in the
  ADR.

### 4.6 `isel` (PIC14) and `isel-pic18`

**Address materialization.** A `Value::Global(f)` where `f` is a function
(currently a hard `addrs.get(f)` panic) becomes the two literal bytes of the
label: byte 0 = `MOVLW LOW(f)`, byte 1 = `MOVLW HIGH(f)` (PIC14: word address
as `parse_lit`; PIC18: byte address, same `parse_lit`). This is the address
value that will later match the chain's compare. Needed in: `emit_load_byte`
(PIC14), `emit_move_val_to_slot`/`emit_load_w` (PIC18), and both Store paths.

**Indirect call lowering** (`callees` non-empty): emit the chain of section 3
instead of the direct `CALL func`:
- args are copied inside each matched arm into that candidate's `{f}::{param}`
  slots (identical copy code to today's direct call, parameterized per
  candidate) — the copy only runs for the candidate that matched;
- the PCLATH discipline (PIC14) is the direct-call one, per arm
  (`MOVLW PAGE(f); MOVWF PCLATH; CALL f; emit_pclath_restore(f)`);
  `emit_pclath_restore` already consults the pass-B page map, which has every
  candidate's page;
- `self.bsr = None` after each arm's `CALL` (PIC18), exactly as today;
- unmatched falls into `L_trap: GOTO L_trap` (deterministic, not silent);
- the retval copy to `dst` sits after `L_done`, shared.

**Skip-safety (issue #6).** The compare pairs (`XORLW` then `BTFSC`, then
`GOTO L_next`) never have a memory operand between the flag-set and the skip
target, so the banking pass has nothing to insert between them; the matched
arm's first instruction is a memory op but sits *after* the skip decision.
The arm bodies are the only memory-heavy regions, and they are unconditional
once entered.

**verify_page_fit / pages.** The chain's GOTOs are intra-function; the
`PAGE(f)`/restore pairs are measured in pass A exactly like direct calls.

### 4.7 `asm`

No change: `LOW(f)`/`HIGH(f)`/`PAGE(f)` already resolve label addresses in
pass 2 (word addresses for PIC14, byte addresses for PIC18), and `GOTO`/
`CALL` take labels.

### 4.8 `sim`

No change: every instruction the chains use is already modeled on both
cores.

---

## 5. The recursion and depth acceptance

`callgraph::build` now sees the indirect edges, so:

- a cycle through a function pointer (`void f(void){ fp = g; fp(); }` /
  `void g(void){ ... f(); }`) is rejected by the existing DFS
  ("callgraph: recursion detected"), with a new test proving it;
- depth over the conservative graph is checked against the device's stack
  depth (8 for both targets) by the existing `check_depth`, unchanged.

---

## 6. ISR interaction (why this is a feature and not a footnote)

The HAL's headline callback (`epic_tick`'s `OverflowCallback`) fires from
`TIMER2_IRQHandler`, an ISR. The design therefore requires:

- the `_isr` duplication to also rewrite *value* references (4.4.1);
- the context-restricted candidate sets (4.4.4), so the ISR's chain runs
  against `_isr` copies inside the disjoint ISR region, and the main's chain
  against originals in the main region;
- an e2e fixture where an ISR fires a runtime-selected callback and the
  simulation confirms the `_isr` copy ran.

---

## 7. Testing

| Test | Where | Proves |
|---|---|---|
| parse indirect call from `.ll` (`call void %3`) | irparse tests | sigil distinction survives parse |
| canonical round-trip with `callees` | ir/tests/roundtrip.rs | text boundary stays diffable |
| candidate-set fill + ISR value rewrite | legalize tests | context restriction, `_isr` pointers |
| cycle through fp rejected | callgraph tests | recursion accounting |
| chain shape on PIC14 and PIC18 | isel / isel-pic18 tests | LOW/HIGH compare + per-arm call |
| **e2e acceptance**: fixture with runtime-selected fp (select shape), 3 candidates, both devices | driver/tests | runs to halt in BOTH simulators with the runtime-selected result; gpasm byte-match for PIC14 |
| e2e: ISR fires a callback through a struct-held fp | driver/tests | `_isr` duplication path, both devices |
| e2e: cycle through fp rejected (driver panic) | driver/tests | end-to-end recursion gate |
| full suite + takeoff ritual | CI | regression |

The acceptance fixture is shaped like `fp2.c` (a byte global drives a
`select ptr @f0, ptr @f1`, the call result lands in a `volatile` out global,
the machine halts); the same source compiles for `p16f877a` and `p18f4550`.

---

## 8. Risks

- **Candidate-set size.** A program that address-takes many functions and
  calls indirectly grows linearly. Correct, loud, bounded by flash. The table
  path (3.B) is the future optimization.
- **Conservative depth.** Programs whose indirect calls create long
  conservative chains now fail the 8-level depth check even if the runtime
  path is shallow. Accepted (issue text); recorded in the ADR.
- **Const fp tables** (`tbl[idx]()`) still panic — a hard limitation boundary
  documented in the ADR's "revisit if".
- **Bogus fp at runtime** traps deterministically (GOTO loop) instead of a
  silent wrong call. A valid C program never produces one (UB otherwise).

---

## 9. Sequencing (implementation order)

1. `ir`: `Call.callees` + serialize/parse + round-trip tests.
2. `irparse`: sigil-preserving callee + parse tests.
3. `legalize`: address-taken set, ISR value rewrite, context split, fill,
   tests.
4. `callgraph`: indirect edges + cycle/depth tests.
5. `isel` + `isel-pic18`: function-address materialization; the chain
   lowering; unit tests.
6. `driver` e2e: acceptance + ISR + recursion fixtures, both devices.
7. ADR-022 (`docs/adr/ADR-022-indirect-calls.md`) + index line; delete this
   plan in the final commit; takeoff ritual.

No parallel crates needed beyond the obvious isel-pair; keep the PR
slice-shaped (a single PR is fine, the crate order above is the diff order).
