# ADR-035 -- `Switch` is a first-class IR terminator; PIC18 lowers dense switches to PCL jump tables

Status: Accepted 2026-09-19<br>
Decides: epic-cc#479

## Context

`irparse` exploded every LLVM `switch` into `icmp eq` + `brcond`
chains at parse time, so the backend never saw the dispatch shape.
Measured on the `hal-pic18-menu-demo-18f4550` size fixture: five
dense 12..17-case switches in `pic18_irq.c` cost roughly 90-100 flash
words each as chains. PIC18's computed-jump idiom (`ADDWF PCL,F` into
a table of absolute `GOTO` entries, page via `PCLATH`) dispatches the
same shape for about half that.

## Decision

- `ir` gains a `Switch` terminator (`val`, `ty`, `default`, ascending
  `(case value, target)` pairs) with canonical text
  (`switch i16 %v, default %d, cases 0 %a, 1 %b, ...`).
- `irparse` preserves dense-contiguous switches (all values in
  `min..=max` present, at least six cases, 2 bytes or narrower) as
  `Switch` when the caller asks for it (`parse_ll_opts`); the driver
  asks only for PIC18, the only core with table lowering.
- `isel-pic18` tables switches that are dense from any base the cost
  gate accepts; sparse, small, or wide switches chain in place with
  per-edge copies. A constant switch value is a backend panic: fold
  it before isel.
- Table shape: bounds check (with a low-bound reject for nonzero
  bases), then W = 2 + 4*idx accumulated from the index slot with no
  scratch byte, `PCLATH` set from `HIGH()` before the offset math so
  W stays live into `ADDWF PCL,F`. Entries are 2-word absolute
  GOTOs; values below a nonzero base get padding entries targeting
  the default edge. Edges carrying phi copies route through
  per-edge trampolines (copies, then a branch to the target); the
  table itself stays a flat GOTO array.
- The dispatch plus table must share one 256-byte page (`ADDWF PCL,F`
  modifies `PCL` only): a `.pcltbl` marker makes `assemble_pic18`
  pad NOPs at the dispatch's `.pclalign` anchor to push a straddling
  block onto the next page, inside the existing fixpoint, and assert
  adjacency, span, and the page fit once placed.

## Alternatives considered

- Peephole the emitted chains back into tables: fragile (the shape is
  lost at text level) and unable to satisfy the page constraint, which
  is a placement fact.
- Preserve switches for every core and panic on PIC14: a capability
  regression for dense switches on PIC14, which compile as chains
  today.
- `BRA`-entry tables (1 word per entry): saves 17 words per table but
  couples the table to a 1 KB window of every case body, which code
  placement cannot promise; rejected for the same reason the manual
  forbids silent layout assumptions.

## Consequences

- Seven crates touch the new variant; the shared passes treat it
  conservatively (`alloc` liveness uses the value and walks all
  targets; `legalize` leaves it).
- The `hal-pic18-menu-demo-18f4550` ladder entry measures 15470 to
  14726 flash words; the `switch_dense` e2e fixture behavior-gates
  the table (bounds, offset math, trampoline copies) through the
  simulator on every run.
