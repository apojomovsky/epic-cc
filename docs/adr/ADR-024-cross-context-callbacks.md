# ADR-024 -- Cross-context stored callbacks (store in main, invoke in the ISR)

**Status:** Accepted 2026-08-25<br>
**Decides:** `epic-cc#137`<br>
**Parent:** `docs/adr/ADR-022-indirect-calls.md`; `docs/superpowers/specs/2026-08-25-cross-context-callbacks-design.md`

## Decision

* The ISR context (the candidate split and the `_isr` duplication) is extended
  with **global store/load visibility edges**: a defined function whose address
  is stored into a global the ISR context reads joins the ISR context, so an
  ISR indirect call site fed by that global gets the callback as a candidate
  and the callback is `_isr`-duplicated when main also reaches it.
* A store whose value is one of the enclosing function's parameters is resolved
  one hop through call sites: a call passing a named function as that argument
  creates the edge, and the call-site argument is rewritten to the `_isr` copy
  (the `EPIC_GPIO_RegisterChangeCallback(on_rb_change)` shape).
* A store whose value is opaque (a register from a struct-field load through a
  pointer, e.g. `g_t0_overflow_cb = h->OverflowCallback`) cannot be resolved:
  the ISR site it feeds gets an empty `callees` and lowers to the
  deterministic trap loop, replacing the previous compile-time panic.
* A store outside the ISR context whose value is a duplicated function and
  whose target is a global the ISR READS is rewritten to the `_isr` copy, so
  the ISR loads and dispatches the copy inside the disjoint ISR region. The
  predicate is ISR-read (loads, memcpy sources), not merely ISR-visible: a
  global the ISR only writes never feeds an ISR call, and rewriting a store
  into it would break the epic-cc#73 fixture (main's store must stay on the
  original).
* isel and isel-pic18 treat an indirect call site (numeric `func`) with empty
  `callees` as a trap, never a direct call: the direct path is only for a
  genuinely-named target.

## Rationale

ADR-022's candidate split derives the ISR context from direct calls and
address-*value* edges. A callback stored by main into a global the ISR reads
has no static edge from the ISR to the callback (the store's value edge is
`main -> callback`; the ISR's load carries no value), so the ISR site got
empty `callees` and isel panicked on the numeric register name. The HAL's
real shape (`EPIC_TIMER0_Init` stores into `g_t0_overflow_cb`,
`TIMER0_IRQHandler` loads and invokes it) is exactly this cross-context
pattern, and it is compiled out under `EPIC_AT` today only because the
backend could not compile it.

The named-store edge is sound: the ISR can only reach a callback through a
global it actually reads that some store writes. Adding those targets to the
ISR context is ADR-022's stated conservatism ("any indirect call may reach
any address-taken function") applied to the store boundary. The shared-function
duplication covers the newly ISR-reachable functions, so the allocator's
disjoint-region guarantee holds.

The trap for opaque stores is not a compromise: putting the main-context
originals in the ISR chain would let the ISR run a main-region frame and
clobber a live main context, a silent miscompile. Trap is the ADR-022 posture
("unmatched fp falls into a deterministic trap loop") applied to an unknowable
value, and it unblocks the HAL builds (they compile without the panic; where
the driver is inlined into the caller, the store becomes a named literal and
the callback fully dispatches).

## Alternatives rejected

* **Whole-address-taken-set ISR candidates always.** The disjoint-ISR-region
  invariant of ADR-013 requires the ISR to never reference a main-context
  frame; without the duplication step, the allocator would reassign a shared
  function's frame into the ISR region while main also calls it (re-entrancy
  clobber). Widening for opaque stores is the same hazard in disguise.
* **Silently dropping the ISR call.** Would compile away real HAL behavior;
  trapping is the honest fallback.

## Known trade-offs

* **Opaque store flows trap.** A program whose callback really does flow
  through a runtime value (a struct-field load through a pointer) does not run
  until per-global value-flow tracking lands. Deterministic and loud, never a
  miscompile.
* **Param hop is one level.** A param stored into an ISR-read global is
  resolved only through direct call sites passing a named function; a
  param-of-a-param chain stays opaque and traps.

## Revisit if

* Per-global value-flow tracking lands, narrowing the opaque-store trap to a
  real chain.
* The HAL needs a callback stored through a multi-hop param chain.
