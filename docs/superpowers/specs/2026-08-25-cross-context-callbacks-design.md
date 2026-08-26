# Cross-context stored callbacks (store in main, invoke in the ISR) -- Design

**Status:** draft (pending approval)
**Date:** 2026-08-25
**Parent:** `docs/adr/ADR-022-indirect-calls.md` (epic-cc#73); ADR-002 (whole-program), ADR-003 (static allocation)
**Ticket:** `epic-cc#137`
**Scope:** `epic-cc` compiler only (legalize, isel, isel-pic18, driver e2e); the HAL side stays in `apojomovsky/epic-hal#67`.

---

## 1. Goal and non-goals

**Goal:** a peripheral callback whose address is stored by `main`-context code and
invoked from an ISR through a global compiles and runs on both `p16f877a` (PIC14) and
`p18f4550` (PIC18), with the ISR executing a callback copy in the disjoint ISR region.
This is the HAL's real shape (`EPIC_TIMER0_Init` stores into `g_t0_overflow_cb` /
`g_t0_storage`, `TIMER0_IRQHandler` loads and invokes it; `EPIC_GPIO_RegisterChangeCallback`
/ `RB_IRQHandler`), which `epic-cc#73`'s fixture does not cover: there the store happens
inside the ISR itself.

**Non-goals (v1):**

- Stores whose callback value flows through a runtime register with no statically
  visible function address (`g_cb = h->OverflowCallback` where `h` is a param): the
  value is opaque to the compiler, so the ISR site cannot be given candidates for it
  soundly; the site lowers to the deterministic trap loop (below), never a compile
  panic.
- Callbacks stored into a global through an unresolvable pointer chain (a pointer
  stored in a global whose initializer is itself runtime-computed): out of scope,
  same trap fallback.
- `const` flash tables of function pointers: unchanged (panic at parse, ADR-022).
- Runtime address-to-function `inttoptr` (#117): unchanged.

## 2. Empirical ground truth (clang 20.1.8, `-target msp430 -O1`)

The issue's repro (`cross_ctx.c`, volatile global):

```llvm
@out = global i8 0
@g_cb = global ptr null          ; volatile cb_t
define void @main(void) {
  store ptr @on_event, ptr @g_cb ; <-- the address-taken edge lives HERE
  store volatile i8 17, ptr @out
  ret void
}
define void @isr(void) {          ; interrupt(0)
  %1 = load volatile ptr, ptr @g_cb
  %2 = icmp eq ptr %1, null
  br i1 %2, label %5, label %3
3:
  %4 = load volatile ptr, ptr @g_cb
  call void %4()                  ; <-- numeric func, no !callees, no static target
  br label %5
 5:
  ret void
}
```

`legalize::fill_indirect_callees` computes `isr_ctx` as the reachability of the ISR
over direct calls + address-value edges. The store `store @on_event, @g_cb` lives in
`main`, so the edge `main -> on_event` enters the main context; the ISR's *load* of
`g_cb` carries no static value, so `on_event` never enters `isr_ctx` and the call
site gets `callees = {}`. isel then falls into the direct-call path with the numeric
register name and panics `isel: call to unknown function @4`
(`crates/isel/src/lib.rs:1761`).

## 2. Approaches considered

### A -- Global store/load visibility edges (chosen)

The ISR context is extended with the globals it reads: for every function in the
(reachability) ISR context, collect the globals its `load`/`store`/`gep`/`memcpy`
targets. A function whose address is stored into one of those **ISR-visible globals**
from anywhere in the module becomes a potential ISR-context indirect target.

Concretely (all in `legalize`, shared by the duplication pass and the callee-fill
pass):

1. **Compute `isr_reach`** exactly as today: closure over direct CALL edges and
   address-value edges from the ISR roots.
2. **Compute `isr_globals`**: globals referenced (load/store/gep/memcpy) by any
   function in `isr_reach`, resolving pointer operands one level through GEPs
   (a `gep @g +k` load/store reaches `g`; a `gep %r +k` whose `%r` is a
   `gep @g` chain resolves to `g` too). A global holding a constant global
   pointer (`g_t0_handle = &g_t0_storage`) is not reachable today: pointer
   initializers parse as zeroinit (ADR-022), so the handle's target is only
   visible when the handle is written by a store, which the store-edge scan
   below already covers.
3. **Named store edges**: for each store in the module whose *target* (after the
   same resolution) is an ISR-visible global:
   - if the stored *value* is a defined function `f`, add `f` to the ISR context;
   - if the stored value is one of the enclosing function's parameters (the
     `EPIC_GPIO_RegisterChangeCallback(on_rb_change)` shape: the callback arrives
     as a param and is stored into `s_rb_change_callback`, which the ISR invokes),
     resolve one hop through call sites: any call passing a named function as that
     argument creates the edge, and the argument is rewritten to the `_isr` copy
     when the target is duplicated (the rewrite lands at the call site, so the ISR
     loads the copy's address);
   - otherwise the value is opaque (a register from a struct-field load through a
     pointer, e.g. `g_t0_overflow_cb = h->OverflowCallback`): the ISR site it feeds
     gets no candidates and lowers to the deterministic trap loop (see 3.2), never
     a compile panic. This is not a compromise: putting the main-context originals
     in the ISR chain would let the ISR run a main-region frame and clobber a live
     main context, a silent miscompile. Trap is the ADR-022 posture ("unmatched fp
     falls into a deterministic trap loop") applied to an unknowable value, and it
     replaces the current compile-time panic.
4. **Cross-context store rewrite**: after `duplicate_isr_shared` creates the `_isr`
   copies, a store *outside* the ISR context (i.e. in main-side code) whose value is
   a duplicated function and whose target is an ISR-visible global is rewritten to
   the `_isr` copy, exactly like the existing ISR-side value rewrite (including a
   call-site argument that forwards into such a store, per the param hop above).
   The ISR then loads and dispatches the copy's address; the main-side original
   stays main-only.

The candidate split then uses the extended context only on the ISR side: an
ISR-context indirect call site gets `callees = A ∩ isr_ctx` (the stored callback is
a candidate and its copy runs in the disjoint ISR region); a main-context site keeps
today's `A - isr_ctx` (its candidates never lose anything, and a main-side call
through the same global gets empty callees and lowers to the trap loop (no main-side `_isr` dispatch).

*Why the named-store edge is sound.* The ISR can only reach a callback through a
global it actually reads (step 2) that some store writes (step 3). Adding those
targets to the ISR context is exactly "any indirect call may reach any
address-taken function that can flow to it", which is ADR-022's stated conservatism
applied to the store boundary. The shared-function duplication covers the newly
ISR-reachable functions, so the allocator's disjoint-region guarantee holds.

*Why the trap for opaque stores.* The value is a runtime quantity the compiler
cannot see; a chain over guesses is either dead (never matches) or unsafe (originals
in the ISR region). Trap is deterministic and loud, and unblocks the HAL builds: the
HAL registers callbacks through params (`g_t0_overflow_cb = h->OverflowCallback`),
so the 87xa/88x builds compile today only because the callback is compiled out under
`EPIC_AT`; with this change they compile without the panic. Where the driver is
inlined into the caller (a single-TU build, the shape of the HAL sim tests), the
store becomes a named literal and the callback fully dispatches.

### B. Whole-address-taken-set ISR candidates always (rejected)

Make every ISR indirect call's candidates the whole address-taken set. Rejected:
the disjoint-ISR-region invariant of ADR-013 requires the ISR to never reference a
main-context frame; without the duplication step, the allocator would reassign a
shared function's frame into the ISR region while main also calls it (re-entrancy
clobber). The named-store/opaque split above duplicates exactly the newly reachable
shared functions, keeping the region guarantee with a bounded chain.

### C. Requires the callback to be declared `noreturn`/synthetic (rejected)

Silently dropping the ISR call would compile away real HAL behavior; trapping is the
honest fallback.

## 3. Pipeline changes

### 3.1 `legalize`

- `duplicate_isr_shared`: use the extended ISR context (steps 1-3) for the
  shared/dup decision, and add the cross-context store rewrite (step 4). The
  existing ISR-side value rewrite is unchanged. The `assert!(!isr_ctx.contains
  ("main"))` guard stays (an opaque store of `main`'s address with the ISR reaching
  it still fails loudly).
- `fill_indirect_callees`: compute the extended ISR context with the same helper and
  use it for ISR-side candidate filtering. Main-side filtering unchanged.
- Shared helper: `fn isr_context_and_globals(m) -> (HashSet<String>, HashSet<String>)`
  the extended context + the ISR-visible global set, computed identically in both
  passes (single decision site).

### 3.2 `isel` (PIC14) and `isel-pic18`

- In `emit_call`, an indirect call site (numeric `func`) whose `callees` is empty no
  longer falls into the direct-call path: it emits the deterministic trap loop (the
  exact block the chain already emits for a non-matching pointer). The direct path is
  now only for a genuinely-named target.
- `crates/callgraph`: unchanged (it consumes `callees`, one edge per candidate; the
  depth/recursion checks get the new edges automatically).

### 3.3 Tests

- `crates/legalize/tests`: store-in-main, call-in-ISR module: the ISR site gets the
  stored callback as a candidate; a shared stored callback gets `_isr`-duplicated
  and the main store rewritten to the copy; a main-only stored callback stays
  main-only (no duplication, no ISR candidate).
- `crates/driver/tests`: e2e `cross_ctx` fixture (store in main, fire in the ISR)
  on both `p16f877a` and `p18f4550` simulators, mirroring `indirect_call_isr_e2e`
  (wait for a RAM marker, `fire_interrupt`, assert the callback wrote the ISR's
  value and the machine halts). Existing `indirect_call_isr_e2e` unchanged.
- A compile-only case asserting the opaque-shape module builds to HEX (no panic).

### 3.4 Docs

- `docs/adr/ADR-024-cross-context-callbacks.md` (decisions above, rejected
  alternatives, revisit-if: register-only flows, per-site alias analysis).
- Index line in `docs/03-decisions.md`.

## 4. Acceptance

1. The issue's repro compiles to HEX for `--target 16F877A` and `--target 18F4550`
   (no `isel: call to unknown function` panic) and runs on the simulator with the
   callback executing.
2. e2e on both cores proves the main-stored callback runs in the ISR and writes the
   expected value.
3. `indirect_call_isr_e2e` remains green.
4. Call-depth and recursion checks still hold (new edges enter the call graph
   conservatively).
5. Full suite green, takeoff ritual clean.

## 5. Known limitations (documented, not silent)

- Opaque store flows (a runtime parameter written into an ISR-visible global) trap
  at the ISR call site when the value cannot be named. Correct, deterministic, but
  a program whose callback really does flow through a runtime value does not run
  until per-global value-flow tracking lands.
- A shared callback stored through a runtime value cannot be rewritten to the
  `_isr` copy; the ISR site traps rather than guess. If the callback is also
  directly called by main, the runtime value is the main-copy address and the trap
  is the honest outcome until the flow becomes visible.
