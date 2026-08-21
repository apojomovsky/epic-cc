# CC-4 Inline Assembly — Design

**Status:** draft (pending user approval of sections below)<br>
**Date:** 2026-08-21<br>
**Parent:** `docs/31-ecosystem-integration-design.md` D-3 (four-rung ladder)<br>
**Scope:** rungs 1–3 only. Rung 4 (memory operands) deferred. No `.asm` file inputs.

---

## 1. Goal and non-goals

**Goal:** epic-cc becomes a standalone PIC toolchain that can build real firmware containing hand-written assembly. The compiler understands naked functions, intrinsic single-instruction ops, module-level asm blobs, and opaque statement-level `asm volatile("...")` blocks, emitting verbatim assembly on both PIC14 and PIC18 with conservative clobber assumptions so banking/overlay remain correct.

**Non-goals (v1):**

* Rung 4 memory-operand substitution (`%0`, `"+m"(x)`). Deferred per D-3 and user choice; if built later it reuses the same `Inst::Asm` plumbing.
* Raw `.asm` compile-unit inputs. Naked functions already give an opaque whole-routine escape hatch that participates in callgraph/overlay; `.asm` blobs would be fully opaque (pinned scratch, no depth check).
* Register-constraint operands (`"=r"`). Front end rejects them loudly (panics-are-the-error-surface).
* Fine-grained clobber modeling. Only `"memory"` and `"cc"` pass clang on msp430; every block is assumed to clobber `W`, `STATUS`, and the bank/IRP state.

---

## 2. Empirical ground truth (probed 2026-08-21, pinned clang 20.1.8, `docs/31` flags)

| C form | `.ll` |
|---|---|
| file-scope `asm("...")` | `module asm "..."` at module top |
| `asm volatile("nop")` inside function | `call void asm sideeffect "nop", ""()` |
| `asm volatile("bcf INTCON, 7")` / `"bsf INTCON, 7"` | same, with that string |
| `asm volatile("nop" ::: "memory")` | `"~{memory}"` clobber |
| `__attribute__((naked))` | `naked noinline` attribute + body `call void asm ...` + `unreachable` |
| `asm("movf %1, w" : "+m"(t) : "m"(y))` (rung 4) | `call void asm sideeffect "movf $1, w\0A addwf $0, f", "=*m,*m,*m"(ptr @t, ptr @y, ptr @t)` — not v1 |
| `asm("movwf %0" : "+r"(a))` | `call i8 asm sideeffect "movwf $0", "=r,0"(i8 1)` — must be rejected |

All three v1 cases already preserve volatile ordering against surrounding accesses (probed).

---

## 3. Approaches considered

### A — IR `Inst::Asm` node with verbatim emission (recommended)

Add `Module.module_asm: Vec<String>`, `Func.naked: bool`, and `Inst::Asm { template: String, clobbers_memory: bool }`. `irparse` lifts the three `.ll` forms into those fields; `isel`/`isel-pic18` emit them verbatim; `banking` treats each block as a barrier (no BANKSEL inside, reset state after). Intrinsics are either `Inst::Asm` with a single known template or a tiny `Inst::Intrinsic` enum — either way they are understood, not opaque.

*Pros:* D-3's design, keeps callgraph + alloc correct, both backends share the same IR, rung 4 later is just extra fields on the same node. *Cons:* touches every crate on the rung-3 path (first time that path exists).

### B — Driver-level text splicing (no IR node)

Driver extracts `module asm "..."` and `call void asm ...` strings from the merged `.ll` text and splices them verbatim into the final `.asm` output, bypassing `ir` entirely.

*Pros:* fewer crates touched. *Cons:* callgraph and alloc never see naked functions (depth check wrong, overlay wrong for scratch), banking cannot be taught to barrier around spliced text, bisectability breaks (IR text no longer tells you where the asm lives). Rejected: D-3's naked-vs-blob rationale applies directly.

### C — Intrinsics-only

Only support the fixed `__epic_*` intrinsics; reject arbitrary assembly strings.

*Pros:* trivial to lower, no barrier logic. *Cons:* does not cover the `bcf/bsf INTCON` idiom or any user hand-written sequence; fails the "standalone compiler" promise. Rejected as insufficient for D-3's rung 3.

**Chosen: A.** With a minimal intrinsic set on top.

---

## 4. IR changes (`crates/ir`)

```rust
pub struct Module {
    pub globals: Vec<Global>,
    pub funcs: Vec<Func>,
    pub module_asm: Vec<String>, // v1: verbatim lines from `module asm "..."` (one entry per directive)
}

pub struct Func {
    pub name: String,
    pub ret: Option<Ty>,
    pub params: Vec<Param>,
    pub blocks: Vec<Block>,
    pub isr: bool,
    pub naked: bool,            // v1
}

pub enum Inst {
    // existing...
    Asm(Asm),                   // v1
}

pub struct Asm {
    pub template: String,       // decoded LLVM asm string (escapes resolved, "\0A" -> "\n")
    pub clobbers_memory: bool,  // true iff "~{memory}" present
    // no operands in v1; rung 4 would add Vec<AsmOperand>
}
```

**Serialization:** `module_asm` as `module_asm "..."` lines before globals; `Func.naked` as `[naked]` marker alongside `[isr]` (order `[isr] [naked]`); `Asm` as `asm "template" [memory]` (quoting mirrors `load`/`store` ptr forms). Old text without those markers still parses.

**`is_runtime_routine`** unchanged; naked and runtime-routine are orthogonal.

---

## 5. Front end (`crates/irparse`)

* `parse_ll` collects every `module asm "..."` line (decode LLVM string escapes per `parse_string_literal` conventions) into `Module.module_asm`. Multiple lines are kept order-preserving; `llvm-link` merging concatenates them with `\0A`? Probe shows single concatenated content — handle both shapes by splitting on the LLVM `\0A` escape after decoding.
* For each function definition line, scan the attribute list for `naked` → `Func.naked = true`. Preserve other attributes.
* For each `call ... asm sideeffect "...", "..." (...)` line inside a function body:
  * Parse the two quoted strings: template and constraints (`""` or `"~{memory},~{cc}"` or `"=*m,*m"` etc.).
  * If constraints contain `=r`, `r`, `m` without `*`, `X`, etc. beyond the allowed `*m` memory form — panic with `"asm: register constraints are not supported on PIC (found \"=r\"); use \"*m\" memory operands or no operands"`. In v1 any non-empty operand list panics with `"asm with operands is not supported in this build (rung 4 deferred); use naked functions or opaque asm(\"...\")"` — this distances the error from a normal compilation path.
  * Otherwise strip the `asm sideeffect` wrapper, decode the template escapes (`\0A` → `\n`, `\"` → `"`, `\\` → `\`), and emit `Inst::Asm { template, clobbers_memory }`. The call's `tail`/`nounwind`/`!srcloc` are ignored.
  * `clobbers_memory = constraints.contains("~{memory}")`. `"~{cc}"` is ignored (no condition-code state to preserve on PIC, but accepted).
  * A naked function's terminal `unreachable` after its asm calls is not emitted as an IR terminator; the function's last block ends with the final `Asm`. The IR verifier should accept a naked function whose last inst is `Asm` (instead of `Ret`/`Br`).

`sanitize_symbols` runs before `parse_ll` — it leaves `module asm` string content untouched (already skips `"`-quoted text) and does not sanitize inside asm templates.

---

## 6. Driver header and intrinsics (`crates/driver`)

Extend `epic-cc.h`:

```c
#define EPIC_NAKED __attribute__((naked))
/* rungs 2 — minimal set, PIC14 & PIC18 share mnemonics where encoding overlaps */
#define __epic_nop()     asm volatile("nop")
#define __epic_clrwdt()  asm volatile("clrwdt")
#define __epic_sleep()   asm volatile("sleep")
#define __epic_di()      asm volatile("bcf INTCON, 7")   /* GIE off — PIC14 name; PIC18 maps to INTCON,GIE */
#define __epic_ei()      asm volatile("bsf INTCON, 7")
```

For PIC18 the same `INTCON` spelling still assembles (INTCON exists on both devices at different addresses, but the mnemonic is portable for the barrier use case); if the encoding diverges, the header can `#ifdef` on a device define injected by the driver (`-D__EPIC_PIC18__`).

Intrinsics are **header-only**: they expand to opaque `asm volatile("nop")` blocks, so they travel the same `Inst::Asm` path. No special `Inst::Intrinsic` needed in v1; the backend emits them identically. This keeps the implementation one path. A dedicated `Inst::Intrinsic` can be added later if an intrinsic ever needs optimization understanding.

`.asm` file inputs are not accepted in v1; the CLI rejects `*.asm`/`*.s` with a precise message directing to `EPIC_NAKED`.

---

## 7. Allocation and call graph (`crates/alloc`, `crates/callgraph`)

* `callgraph::build` already walks every `Inst::Call` — `Inst::Asm` adds no edges, so the graph is unchanged. Naked functions are still nodes (they have a `Func` entry) and still participate in `max_depth`. A `CALL` into a naked function counts toward the 8-deep limit just like any other call — this is D-3's load-bearing property over a file-scope blob.
* `alloc::allocate` assigns a frame to every `Func`, including `naked: true`. A naked function's locals are whatever `alloca`/`def_width` finds — in practice naked bodies contain only `Asm` and no SSA defs, so the frame is empty or holds only explicit temporaries, but the overlay slot is still reserved and overlaid correctly against its non-concurrent siblings.
* `def_width(Inst::Asm(_)) -> None` — assembly defines no IR value.
* Module-level `module_asm` defines no addresses.

---

## 8. Code generation (`crates/isel`, `crates/isel-pic18`)

Both backends share the same policy; `isel-pic18` is not PIC14-specific here.

* `Module.module_asm` is emitted verbatim at the very top of the `.asm` output, before any function label, one line per entry (already decoded). The assembler already accepts free-form directives there.
* For a `Func { naked: true, .. }`: emit the function label, then for each `Inst::Asm` in block order emit its `template` lines verbatim (split on `\n`). No prologue, no phi copies, no `RETURN` synthesis — the user's assembly must include its own `return`/`retfie`/`goto`. This matches clang's `naked` contract and keeps the backend's page-assignment word count exact.
* For a non-naked `Asm` inside a normal function body: emit the template lines verbatim at that point in the block's instruction stream. Treat the block as a compiler memory barrier: no reordering of loads/stores across it (phase ordering already preserves block order; document that `clobbers_memory` is the barrier marker).
* **Conservative clobbering:** every `Asm` block is assumed to clobber `W`, `STATUS` (hence bank/IRP), and `FSR`/`PCLATH` where relevant. The backend therefore:
  * does not keep any live-in-`W` value across the block (spill before, reload after if needed);
  * does not assume any bank/IRP after the block — `banking` handles PIC14, `isel-pic18` handles `BSR`/`ACCESS` bits natively;
  * does not elide any `STATUS, RP0/RP1` (PIC14) or `BSR` (PIC18) setup after it beyond what the next instruction's own inference does.

---

## 9. Banking and peephole (`crates/banking`, `crates/peephole`)

* `banking::assign_banks` never inserts a `BANKSEL` (i.e. `BCF/BSF STATUS,5/6`) **inside** an `Asm` template. The template is opaque; splitting it would change the skip target of a `BTFSC/BTFSS/INCFSZ/DECFSZ` that the user wrote. Instead the pass tracks the bank as `UNKNOWN` on entry to the block and `UNKNOWN` on exit, so the next banked operand after the block gets a full `BANKSEL` (both RP bits re-established). This is the `MOVWF STATUS` policy generalized: any opaque block makes the bank unknowable.
* `peephole::optimize` does not cross `Asm` boundaries. Patterns that would otherwise elide a `BANKSEL` or merge instructions are disabled when an `Asm` line sits between them.

---

## 10. Error handling

All panics use the project's panics-are-the-error-surface rule with precise messages:

* `asm with operands is not supported in this build (rung 4 deferred)` — when the constraint string is non-empty and not just clobbers (`~{memory},~{cc}`).
* `asm: register constraints are not supported on PIC; found "=r" (only "*m" memory and no-operand forms are valid on this target)` — when any operand constraint is not `*m`.
* `naked function '<name>' must not have C parameters or return value` — optional tightening; clang already warns but epic-cc should panic if a user writes `EPIC_NAKED int foo(int x)`.
* `asm inside a naked function must be the entire body` — only relevant if someone mixes `Asm` with `Bin`/`Load`/etc inside a naked function; v1 panics rather than emits a half-naked body.

---

## 11. Testing

* `irparse` unit tests: `module asm` round-trip, `naked` attribute, opaque `asm sideeffect "nop"` → `Inst::Asm`, `~{memory}` → `clobbers_memory`, register constraint rejection, operand rejection.
* `ir` serialize/parse round-trip for the new fields.
* `isel` and `isel-pic18` unit tests: naked emission (label + verbatim lines, no prologue), inline opaque emission order, `W` spill discipline (if measured), and that `verify_page_fit` counts verbatim words.
* `banking` test: `BANKSEL` never inside an asm block, unknown after.
* Driver e2e fixtures (4–6): `asm_naked` (PIC14+PIC18), `asm_opaque` (GIE guard around a counter), `asm_module` (`module asm` directive), `asm_intrinsic` (`__epic_nop/clrwdt/sleep/di/ei`), each with committed `.hex` via `driver/tests` golden path.
* Negative driver tests: `asm_with_operands` and `asm_reg_constraint` both panic with the expected substring.

---

## 12. Sequencing and dependencies

Single crate-ordered pass; no parallelism needed (cross-crate contract is one `Inst::Asm` shape):

1. `crates/ir` — new fields + text format.
2. `crates/irparse` — lift `module asm`, `naked`, and `call asm sideeffect`.
3. `crates/alloc` + `crates/callgraph` — recognize `Asm` (no edges, no def).
4. `crates/isel` + `crates/isel-pic18` — verbatim emission (naked vs inline) with barrier/conservative-clobber comments.
5. `crates/banking` + `crates/peephole` — barrier rules.
6. `crates/driver` — `epic-cc.h` extensions (header-only intrinsics), CLI rejection of `*.asm`.
7. Fixtures + negative tests, `make test`.

Porting `alloc::tests` overlay expectations is not needed — naked frames are empty, so existing overlay tests are unaffected; add one naked-overlay test.

---
