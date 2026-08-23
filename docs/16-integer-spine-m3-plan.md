# Integer Spine — Milestone 3: Overlay Allocation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Non-interfering functions share RAM frames. The `alloc` stage (stage 6, its proper home per the design) assigns **every** address — globals and all locals — using call-graph overlay; `isel` consumes the complete address map instead of allocating slots itself. A program with two sibling functions carrying large frames runs correctly and demonstrably uses less RAM than the sum of their frames.

**Architecture:** `callgraph` emits a parseable edge list; `alloc` reads the IR + edges, computes each function's local-byte demand, assigns frames with the classic interval overlay (`base(f) = max over ancestors of (base(a) + locals(a))`), and emits a complete address map (`global` + `local` lines); `isel` drops its internal slot allocator and looks every value up in the map. Common RAM (0x70–0x7F) is **not** used for locals in this milestone — all locals live in overlay-assigned bank-0 frames, keeping the overlay single-region and correct; the imaginary-registers optimization returns in a later milestone.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

**Spec:** [`docs/12-backend-design.md`](../12-backend-design.md) §2 (stages 5–6: call graph + overlay allocation) and §4 phase 2.

## Global Constraints

- Build/test with `make exec CMD="cargo ..." …`; never `apt install` toolchain deps.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only; GPL never linked.
- Text boundaries: stages communicate via text; the driver may import stage libraries; stages do not import each other. The `alloc` output map is a text artifact consumed by the driver and `isel`.
- New files must be `git add`ed before `make shell  # docker` sees them.
- Unsupported constructs panic loudly, never silently miscompile.

## The overlay algorithm (the load-bearing design)

For a call graph (DAG; recursion is already rejected by `callgraph`):

- `locals_size(f)` = sum of the byte widths of f's params and defined values (i8=1, i16=2), counted once each (the milestone-2 naive per-function demand; phi destinations are defined values).
- `depth_end(f)` = `locals_size(f) + max(0, max over callees c of depth_end(c))` — the RAM span of f's subtree including f's own locals.
- `base(f)` = `max over ancestors a of (base(a) + locals_size(a))` (roots: `base = bank0_start`). This places f's frame after every function that is co-live with f (its ancestors); siblings — never co-live — share the same base.
- Total bank-0 demand = `max over roots of depth_end(root)`; the milestone asserts this is **less than the sum** of the individual functions' demands when there are siblings.
- Each local of f is assigned `base(f) + offset` (offset in IR order, packing values by width).

Layout constants (alloc computes, matching isel's current scheme so nothing else changes): `end_of_globals` = max global addr + its width (even-aligned i16 per milestone-2); `scratch = end_of_globals`, `retval_lo = end_of_globals + 1`, `retval_hi = end_of_globals + 2`, `bank0_start = end_of_globals + 3` (the frame base for roots). All local frames live in bank 0 (`bank0_start .. 0x6F`); the plan's acceptance program must fit.

## Address map text format (extended)

The `alloc` output map gains `local` lines; the driver merges both into one `HashMap<String, u8>` (globals keyed by name, locals keyed `{func}::{name}`):

```
global in 0x20
global out 0x21
local main::%1 0x2A
local main::%2 0x2C
local big_a::%5 0x30
…
```

---

### Task 1: `callgraph` — parseable edge output

**Files:**
- Modify: `crates/callgraph/src/lib.rs`, `crates/callgraph/src/bin/callgraph.rs`
- Test: `crates/callgraph/tests/graph.rs` (extend)

**Interfaces:**
- Produces: `pub fn edges_text(g: &CallGraph) -> String` — one `edge <caller> <callee>` line per edge, then `depth <max_depth>`; the binary writes it. `alloc` (Task 2) parses this text.

- [ ] **Step 1: Extend the failing test** — assert `edges_text` for a main→a, main→b graph contains `edge main a`, `edge main b`, `depth 2`.
- [ ] **Step 2: Run to verify it fails** — `make test CRATE=callgraph  # docker: cargo test -p callgraph`.
- [ ] **Step 3: Implement** — add `edges_text`; the binary writes it (replacing/extending the current output; keep `depth`).
- [ ] **Step 4: Run to verify it passes**.
- [ ] **Step 5: Commit** — `git commit -m "feat(callgraph): emit parseable edge list"`.

---

### Task 2: `alloc` — overlay frame assignment and local addresses

**Files:**
- Modify: `crates/alloc/src/lib.rs`, `crates/alloc/src/bin/alloc.rs`
- Test: `crates/alloc/tests/alloc.rs` (extend)

**Interfaces:**
- Consumes: `ir::Module`, the callgraph edge text (passed in as a string).
- Produces:
  - `pub struct AllocLayout { pub globals: HashMap<String, u8>, pub locals: HashMap<String, u8>, pub total_bank0: u16 }` (locals keyed `{func}::{name}`).
  - `pub fn allocate(m: &Module, edges_text: &str) -> AllocLayout` — implements the overlay algorithm above. Globals as milestone-2 (sequential, even-aligned i16). Locals per frame.
  - `pub fn map_text(l: &AllocLayout) -> String` — the `global`/`local` lines.
- The binary reads IR text + a `.cg` file + writes the map. (Driver wires it in Task 3.)

- [ ] **Step 1: Extend the failing test** — the overlay math: a module with `main` calling `a` and `b`, where a and b each have a couple of i16 locals; assert (a) a's and b's locals share addresses (same base), (b) main's locals are at a base that doesn't overlap a's, (c) `total_bank0` < sum of the three functions' individual demands.
- [ ] **Step 2: Run to verify it fails**.
- [ ] **Step 3: Implement** — parse edges into a call graph; compute `locals_size` per function from the IR (params + defined values by `Ty::bytes()`); compute `depth_end`, `base` (topological/DFS); assign addresses; produce the maps. Globals unchanged. Panic loudly if a frame exceeds bank 0 (`base + locals_size > 0x70`).
- [ ] **Step 4: Run to verify it passes**.
- [ ] **Step 5: Commit** — `git commit -m "feat(alloc): overlay local frames from the call graph"`.

---

### Task 3: `driver` + `isel` — consume the complete address map

**Files:**
- Modify: `crates/driver/src/main.rs`, `crates/isel/src/lib.rs`, `crates/isel/src/bin/isel.rs`
- Test: `crates/isel/tests/isel.rs` (extensively updated), `crates/driver/tests/e2e.rs` (regression: `out = in + 1` still passes)

**Interfaces:**
- Consumes: the complete map (globals + locals) from alloc (Task 2).
- Produces: `isel::select(&m, &addrs)` where `addrs: HashMap<String, u8>` now contains locals too; isel **removes its internal slot allocator** (`slot`/`alloc_slot`/phi pre-reservation/scratch-overlap guards) and looks up every value's address: globals by name, locals by `{func}::{name}`. `scratch`/`retval` stay computed from `end_of_globals` (the map's globals) as today.

- [ ] **Step 1: Extend the failing test** — build an address map that includes locals (hand-construct a `HashMap` with `main::%1 → 0x2A` etc. matching what alloc would emit for the straight-line program), and assert isel emits the same instructions as before using those addresses (no internal allocation).
- [ ] **Step 2: Run to verify it fails** (isel still allocates internally / ignores the local map entries).
- [ ] **Step 3: Implement the isel refactor** — delete the slot allocator; every value lookup goes through the map. Keep the i8/i16 width asserts, the icmp-scratch usage (now from the map-provided layout), call/ret retval handling. Update every isel test that asserted allocator-chosen addresses to use map-provided addresses (addresses become explicit test inputs).
- [ ] **Step 4: Update the driver** — parse both `global` and `local` lines into one map (local key `{func}::{name}`), pass it to `isel::select`. The `add.c` e2e must still pass (`out == 8` for `in == 7`).
- [ ] **Step 5: Run to verify it passes** — `make test  # docker: cargo test --workspace`.
- [ ] **Step 6: Commit** — `git commit -m "feat(isel,driver): consume the complete overlay address map"`.

---

### Task 4: Overlay acceptance — sibling frames share RAM and run correctly

**Files:**
- Create: `crates/driver/tests/fixtures/overlay.c`, `crates/driver/tests/overlay_e2e.rs`
- Test: `crates/alloc/tests/alloc.rs` (extend) or a new `crates/alloc/tests/overlay.rs`

**Interfaces:**
- Consumes: the full pipeline (Task 3) and `pic14_sim`.

- [ ] **Step 1: Write the failing overlay acceptance program**

`crates/driver/tests/fixtures/overlay.c` — two sibling functions with large frames (many simultaneous live i16 locals, kept live by volatile stores to a sink so `-O1` cannot fold them), called sequentially from `main`:

```c
volatile unsigned char in;
volatile unsigned char out;
volatile int sink;

__attribute__((noinline)) static int big_a(int x) {
    int t0 = x + 0, t1 = x + 1, t2 = x + 2, t3 = x + 3;
    int t4 = x + 4, t5 = x + 5, t6 = x + 6, t7 = x + 7;
    sink = t0; sink = t1; sink = t2; sink = t3;
    sink = t4; sink = t5; sink = t6; sink = t7;
    return t0 + t1 + t2 + t3 + t4 + t5 + t6 + t7;
}
__attribute__((noinline)) static int big_b(int x) {
    int u0 = x * 2 + 0, u1 = x * 2 + 1, u2 = x * 2 + 2, u3 = x * 2 + 3;
    int u4 = x * 2 + 4, u5 = x * 2 + 5, u6 = x * 2 + 6, u7 = x * 2 + 7;
    sink = u0; sink = u1; sink = u2; sink = u3;
    sink = u4; sink = u5; sink = u6; sink = u7;
    return u0 + u1 + u2 + u3 + u4 + u5 + u6 + u7;
}
void main(void) {
    out = (unsigned char)(big_a(in) + big_b(in + 1));
}
```

(The exact arithmetic doesn't matter; the acceptance is: (a) the program runs correctly, (b) the address map shows big_a and big_b sharing local addresses. If `-O1` folds too much, adjust the body — more intermediates, or use `in`-dependent expressions — until both functions carry ≥ 16 bytes of simultaneous locals; verify by inspecting the `.ll`.)

- [ ] **Step 2: Write the acceptance test** — `overlay_e2e.rs` runs the driver on `overlay.c`, simulates the HEX with `in = 3`, asserts `out` equals the hand-computed expected value and `halted()`. Separately, a test that runs `alloc` on the overlay program's IR (via the stage binaries or the `alloc` lib) and asserts the local map shows big_a's and big_b's locals **overlapping** (same base region), and `total_bank0` < `locals_size(big_a) + locals_size(big_b) + locals_size(main)`.
- [ ] **Step 3: Run to verify it fails** (before Task 2/3 land, or if the program's frames don't actually overlay), then make it pass. Debug in the responsible stage; keep stage tests green.
- [ ] **Step 4: Run the full suite** — `make test  # docker: cargo test` all green (probe e2e, add.c e2e, overlay e2e).
- [ ] **Step 5: Commit** — `git commit -m "test(e2e): overlay sibling frames share RAM and run correctly"`.

---

### Task 5: gpasm cross-check for the overlay program

**Files:**
- Create: `crates/asm/tests/fixtures/overlay.asm`, `crates/asm/tests/gpasm_overlay.rs`

**Interfaces:**
- Consumes: `asm::assemble_file_to_hex`, `pic14_sim`; mirrors the milestone-2 `gpasm_probe.rs`.

- [ ] **Step 1: Write the fixture + test** — capture the driver's `.asm` for `overlay.c` (run the stage binaries by hand as in milestone-2 Task 8), fixture it; assert our HEX matches gpasm byte-for-byte and runs correctly (`in = 3` → expected `out`, halted).
- [ ] **Step 2: Run to verify it fails, then passes** — fix any encoding gaps in `crates/asm` (none expected — the instruction set is unchanged; if `BANKSEL` or new mnemonics appear, handle them, but they should not in this milestone).
- [ ] **Step 3: Run the full suite** — all green.
- [ ] **Step 4: Commit** — `git commit -m "test(asm): cross-check overlay program against gpasm"`.

---

## Self-review notes

- **Spec coverage:** milestone 3 implements spec §2 stage 6 (overlay allocation) in its proper home, with isel (stage 7) consuming the allocator's output — the architecture the design mandates. Banking (stage 8) is milestone 4.
- **Deferred (later milestones, panic loudly until then):** common-RAM imaginary registers (locals all in bank-0 frames now); BANKSEL across banks (milestone 4); liveness-based value reuse within a frame (the naive per-value demand is the conservative baseline); pointers/GEP (phase 3).
- **Correctness notes for the implementer:** the interval overlay is exact for call DAGs — verify `base(f) = max over ancestors (base(a)+locals(a))` and that sibling bases are equal; assert no frame exceeds bank 0. The isel refactor must keep the i1/icmp/brcond/select consistency and call/ret protocol intact — the milestone-2 e2e tests (probe, add.c) are the regression net.
- **Type consistency:** `alloc::{AllocLayout, allocate(m, edges_text), map_text}`, `callgraph::edges_text`, `isel::select(m, addrs)` with the extended map — names stable across tasks. The map line formats (`global <name> 0xNN`, `local <func> <name> 0xNN`) are the contract between alloc, the driver, and isel.
