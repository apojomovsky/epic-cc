# ADR-038 -- Dispatch-storage frame policy: per-priority copies, no foreign frames

Status: Accepted 2026-09-22<br>
Decides: epic-cc#569

## Context

A callback registered into a storage global that more than one context
dispatches through (main polling plus a real ISR, or both ISR
priorities) has one address in that storage, and legalize rewrites the
stored spelling to one priority's suffixed copy. Since epic-cc#570,
`fill_indirect_callees` lists that stored copy at every context's
dispatch sites, including sites live in the OTHER priority: a
hi-context site can list the lo context's `f_isr`. At runtime the
higher context then executes a frame that lives in the lower context's
overlay region, and if the lower context was already inside that same
callback when the interrupt hit, one static frame is re-entered. On an
overlay target a static frame is not re-entrant, and the compiler
cannot verify re-entrancy from the source.

## Decision

- A dispatch site may list candidates whose frames it can occupy
  soundly: its own context's members (its own region), main-context
  originals (the main band, disjoint from every ISR region), and, for
  main-context sites, either priority's stored copy (main preempts
  nothing).
- A site live in priority P must not list the other priority's
  suffixed copy. That frame belongs to the other priority's region and
  can be re-entered by it mid-execution. epic-cc#570 admitted the
  shape to keep shared-storage sites non-empty; this ADR documents the
  re-entrancy contract that admission rides on, and the residual
  hazard it leaves.
- The sound program shapes stay: register per-priority spellings (the
  store rewrite then gives each context's dispatch site its own copy,
  and a store feeding both priorities with no single spelling panics,
  as it already did), or confine the dispatch of one storage to a
  single priority.

## Alternatives considered

- Panic on a foreign-priority copy at a dispatch site, mirroring the
  existing dual-copy panic: rejected for now because the candidate
  filter is context-scoped, not storage-scoped. A hi site admits a lo
  stored copy by NAME whenever arities match, even when that site
  dispatches through a different storage that could never hold it, so
  the check as it stands would fire on sound programs. A
  storage-scoped candidate list (candidates keyed by the storage the
  site loads) would make the check exact; that refactor is filed as a
  follow-up and is the trigger to revisit the panic.
- Per-priority storage duplication in legalize (rewrite reads and
  writes so each context dispatches its own global): rejected because
  storage identity is program-visible; main's writes must reach the
  context the program addresses, and splitting one global into two
  silently changes that semantics.

## Consequences

- Programs whose storage is dispatched from both priorities must
  register per-priority spellings, or confine that storage's dispatch
  to one priority. The compiler's observable behavior for the tracked
  shapes is unchanged from epic-cc#570.
- The residual hazard is narrow and documented: a context dispatching
  a foreign copy through a flow the store-edge analysis does not track
  can still re-enter the other priority's frame. Finding those flows
  is the storage-scoped candidate refactor's job.
- epic-cc#467's empty-candidate failure mode stays solved: shared
  storage sites keep their candidates under this contract.
