# ADR-022 -- Indirect calls through function pointers

**Status:** Accepted 2026-08-24<br>
**Decides:** `epic-cc#73`<br>
**Parent:** `docs/superpowers/specs/2026-08-24-indirect-calls-design.md`

## Decision

* An indirect call site (`call %fp(...)`) lowers to an inline compare-and-call
  chain over the whole-program address-taken set: each candidate's two address
  bytes are compared against the fp value, the matched arm copies args and
  CALLs with the direct-call PCLATH discipline, and an unmatched fp falls into
  a deterministic trap loop. The same lowering runs on PIC14 and PIC18.
* The candidate set is the whole-program address-taken set (every function
  whose address appears as a value), split by call-graph context: an
  ISR-context site only references `_isr` copies, a main-context site only the
  originals. `!callees` metadata is never consumed (clang omits it for table
  loads).
* The ISR duplication rewrites function-pointer VALUES (a stored callback, a
  select arm) to the `_isr` copy, and address-taken edges join the ISR-context
  reachability so a stored-only callback is duplicated too.
* `Call` gains a `callees: Vec<String>` candidate list; empty for a direct
  call, non-empty for an indirect one whose `func` is the SSA register name.
  The canonical text round-trips it as a `callees f0 f1` suffix.
* The callgraph emits one edge per candidate, so the depth check and the
  overlay allocator see the conservative whole-program graph. A cycle through
  a function pointer is rejected by the existing DFS.

## Rationale

The hardware has no `CALL` to a computed address on PIC14, and the PIC18
simulator has no PCL-write model, so a computed-PCL dispatch table is not a
portable lowering. A compare-and-call chain uses only instructions both
simulators already model, needs no runtime routine and no new assembler
syntax (`LOW`/`HIGH`/`PAGE` label resolution already exists), and is
deterministic and page-safe. Whole-program compilation makes the address-taken
set exact, so the chain is built over exactly the functions that can be called
indirectly.

The context split is load-bearing: the overlay allocator's disjoint-region
analysis requires that the ISR context never reference a main-context frame.
Without the value rewrite, an ISR's stored callback would point at the
main-context original and the ISR would silently run in the main region's
frames.

## Alternatives rejected

* **Computed-PCL dispatch table (PIC14 only).** The compiler already owns the
  `ADDLW LOW(table); MOVWF PCL` trick in its const readers, but the PIC18
  simulator has no PCL-write model, so PIC18 would need simulator surgery or a
  different lowering anyway. One lowering across cores wins on correctness
  risk. A PIC14-only table can follow as a size optimization.
* **Runtime dispatch library routine.** A shared routine's arguments would
  need a fixed ABI slot arrangement inside the frame overlay, and the chain is
  only ~10 words inline per site.
* **Dispatch on `!callees` metadata.** clang omits it for table loads, so it
  is an optimizer hint, not a correctness guarantee.

## Known trade-offs

* **Conservative depth.** Every candidate is a call-graph edge, so the depth
  check can exceed the 8-level hardware stack and reject a legal program whose
  runtime path is shallow. Accepted: the alternative (per-site exact
  resolution) needs alias analysis we do not have.
* **Linear dispatch.** A call resolving at candidate position `k` costs ~10k
  cycles before the CALL. For the HAL's handful of callbacks this is
  negligible; a program that address-takes many functions and dispatches to a
  late one in a hot loop pays linearly.
* **Const fp tables** (`static const` dispatch tables) still panic at parse.
  The HAL registers callbacks into RAM structs, which is the shape v1 targets.

## Revisit if

* A PIC18 simulator gains a PCL-write model, making a computed-PCL dispatch
  table portable and worth the size win.
* Const fp tables become a real HAL need, requiring global-initializer decode
  of `ptr @f` elements.
