# PIC18 interprocedural BSR exit-bank carry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the MOVLBs on PIC18 that exist only because the tracked BSR state is discarded at every call return, by carrying each callee's provable exit bank into the caller.

**Architecture:** `isel-pic18` already emits each function into its own `Gen` buffer and records every block's end bank. Functions are emitted in reverse topological call order (callees first), each function's RETURN-ending block states are joined into an exit-bank map, and the direct/indirect call arms consult the map instead of clearing unconditionally. Buffers concatenate in original module order, so nothing downstream moves.

**Tech Stack:** Rust (workspace crates), unittest via `cargo test` inside the docker dev image (`make exec` / `make test`), the in-repo PIC18 simulator as the behavioral oracle.

**Spec:** `docs/superpowers/specs/2026-09-22-pic18-exit-bank-carry-design.md`

## Global Constraints

- All builds and tests run in the docker dev image: `make exec CMD='...'` or `make test CRATE=<crate>`. Never install rustup/clang/gpasm on the host.
- Conventional Commits, single line, no trailers, no em-dashes. Scope is usually the crate (`isel-pic18`).
- Comments: why, not what; blocks of 1-8 lines; no iteration narrative; no em-dashes. The pre-push hook enforces this.
- `make check-warnings` must stay clean; `cargo build --workspace --all-targets` fails on any warning.
- The simulator (`pic14_sim::Pic18`) is the oracle for every elided MOVLB; no test may trust the text alone where behavior is claimable.
- `UPDATE_SIZE_BASELINE=1` is used once, deliberately, in Task 4, and the diff may only shrink rows.
- If Task 0's gate shows a negligible elidable population, STOP and report to the human: the ticket closes with data and none of Tasks 1-4 run.

## Review Focus

- A callee textually defined after its caller (forward call): the map lookup must miss and clear, never panic on a missing key. Pinned by Task 2's `a_forward_defined_callee_with_provable_exit_still_carries` and Task 1's order test.
- Runtime routine and naked callees (no `Gen` run, absent from the map): callers must fall back to today's clear. Pinned by Task 2's recipe-call test.
- `tmp{n}` label renumbering when emission order differs from module order: labels are unique and name-resolved; nothing may compare outputs positionally against master. Covered by the full suite in Task 1.
- Valued-return callees and callees whose terminator lowering selects banks (per-edge phi copies): the end state is poisoned, so their exit is unknown and carry does nothing. Expected and conservative; pinned by Task 2's phi-callee test.
- An indirect candidate list mixing known and unknown exits: the trap path and unknown candidates must clear, only unanimous candidates carry, and the shared done label's fall-through (the trap block, bank unknown) must not defeat a unanimous carry. Pinned by Task 3's two tests.

---

### Task 0: Gate measurement on master (no code changes)

**Files:**
- Create: `/tmp/opencode/movlb_gate.py` (throwaway, never committed)

**Interfaces:**
- Consumes: the menu-demo fixture under `crates/driver/tests/fixtures/vendor/hal-pic18-menu-demo`, the driver binary in the dev image.
- Produces: a go/no-go number reported to the human: how many CALL sites have a provable single-bank callee exit.

- [ ] **Step 1: Confirm the size ladder is green on master**

Run: `make test CRATE=driver` (in the worktree; this runs the whole driver suite including `size_regression_e2e`).
Expected: PASS, no baseline movement.

- [ ] **Step 2: Produce the menu-demo asm listing**

Read `crates/driver/tests/size_regression_e2e.rs:220-270` and copy the exact include dirs and source list for the `hal-pic18-menu-demo-18f4550` case. Then compile with the driver's `--emit asm` (check `cargo run -p driver -- --help` for the exact flag spellings) and save the listing inside the workspace so the container can write it:

```bash
make exec CMD='cargo run -p driver -- --emit asm -I <include dirs from the test> <source files from the test> > target/menu-demo.asm'
```

Expected: `target/menu-demo.asm` exists and contains `MOVLB` lines.

- [ ] **Step 3: Count MOVLBs and provable-exit CALL sites**

Write `/tmp/opencode/movlb_gate.py`:

```python
import re, sys

text = open(sys.argv[1]).read().splitlines()
# Function bodies: a label at column 0 starts one, the next column-0 label ends it.
funcs = {}   # name -> list of body lines
order = []
cur = None
for line in text:
    if re.match(r"^[A-Za-z_][A-Za-z0-9_]*:", line):
        cur = line.split(":")[0]
        funcs[cur] = []
        order.append(cur)
    elif cur is not None:
        if line.startswith("    org "):  # vector glue ends a body context
            cur = None
        else:
            funcs[cur].append(line)

def exit_bank(name):
    # The bank each RETURN would leave live: the most recent MOVLB value
    # in the body before that RETURN. Provable only if every return
    # agrees on one value.
    seen = []
    last = None
    for ln in funcs.get(name, []):
        m = re.search(r"MOVLB\s+0x([0-9A-Fa-f])", ln)
        if m:
            last = int(m.group(1), 16)
        if re.search(r"\b(RETURN|RETFIE)\b", ln):
            seen.append(last)
    if seen and all(s is not None for s in seen) and len(set(seen)) == 1:
        return seen[0]
    return None

calls = []
for fname, body in funcs.items():
    for i, ln in enumerate(body):
        m = re.search(r"\bCALL\s+([A-Za-z_][A-Za-z0-9_]*)", ln)
        if m:
            calls.append((fname, i, m.group(1)))

total_movlb = sum(ln.count("MOVLB") for lines in funcs.values() for ln in lines)
provable = [(f, c) for f, _, c in calls if exit_bank(c) is not None]
print(f"functions: {len(funcs)}  MOVLBs: {total_movlb}  CALL sites: {len(calls)}")
print(f"CALL sites with provable single-bank callee exit: {len(provable)}")
for f, c in provable:
    print(f"  {f} -> {c} (exit bank {exit_bank(c)})")
```

Run: `python3 /tmp/opencode/movlb_gate.py target/menu-demo.asm`
Expected: a report. Record `MOVLBs` and the provable-site count.

- [ ] **Step 4: Report and gate**

Report both numbers to the human. Proceed to Task 1 only if the provable-exit CALL site count is at least 10; otherwise STOP: the finding closes epic-cc#495 with data (file the numbers on the ticket via `gh issue comment`, update docs/42's recommendation line, and skip to Task 4's ADR step to record the negative).

### Task 1: Emission-order restructure (behavior-preserving)

**Files:**
- Modify: `crates/isel-pic18/src/lib.rs` (the `select_with_locs` function loop, roughly lines 6230-6945; new free functions near `block_dominators`)
- Test: `crates/isel-pic18/tests/isel_pic18.rs`

**Interfaces:**
- Consumes: `ir::is_runtime_routine(&str) -> bool`, `Func { name, isr, naked, irq_priority, blocks }`, the existing per-function `Gen` construction and `block_end` recording.
- Produces: `fn emission_order<'a>(funcs: &[&'a Func], edges: &HashMap<&str, Vec<&str>>) -> Vec<&'a Func>` and `fn call_edges<'a>(funcs: &[&'a Func]) -> HashMap<&'a str, Vec<&'a str>>`; a `bodies: HashMap<&str, (Vec<String>, Vec<Option<SrcLoc>>)>` keyed by function name that Task 2 reads alongside the new `exits` map.

- [ ] **Step 1: Write the failing order-preservation test**

Append to `crates/isel-pic18/tests/isel_pic18.rs` (same helpers as the join tests: `parse`, `addrs`, `select`, `PIC18F4550`):

```rust
#[test]
fn bodies_concatenate_in_module_order_even_when_emission_reorders() {
    // main calls a helper defined after it. The carry analysis (epic-cc#495)
    // must emit helper first, but the output stream keeps master's order:
    // helper's body may not float above main's.
    let m = parse(
        "global a i8\nglobal b i8\nglobal c i8\nglobal d i8\nglobal e i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @a\n\
             %2 = load i8 @b\n\
             %3 = add i8 %1, %2\n\
             call void @helper()\n\
             store i8 %3 @c\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             %1 = load i8 @d\n\
             store i8 %1 @e\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x21),
        ("c", 0x22),
        ("d", 0x23),
        ("e", 0x24),
        ("main::1", 0x30),
        ("main::2", 0x31),
        ("main::3", 0x32),
        ("helper::1", 0x33),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    let main_at = asm.find("\nmain:").expect("main label");
    let helper_at = asm.find("\nhelper:").expect("helper label");
    assert!(helper_at > main_at, "helper body must stay after main:\n{asm}");
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.ram_mut()[0x20] = 3;
    p.ram_mut()[0x21] = 4;
    p.run(500);
    assert!(p.halted());
    assert_eq!(p.ram()[0x22], 7, "the post-call store must land");
}
```

- [ ] **Step 2: Run the new test and the crate suite**

Run: `make exec CMD='cargo test -p isel-pic18 --test isel_pic18 bodies_concatenate_in_module_order'`
Expected: PASS already (master emits in module order). This test pins the invariant the refactor must keep; it must still pass after Step 4.

Run: `make test CRATE=isel-pic18`
Expected: PASS (baseline).

- [ ] **Step 3: Restructure the loop**

In `select_with_locs`, after the `funcs` sort (line ~6232) and before the per-function loop:

```rust
let edges = call_edges(&funcs);
let order = emission_order(&funcs, &edges);
let mut bodies: HashMap<&str, (Vec<String>, Vec<Option<SrcLoc>>)> = HashMap::new();
```

Add the two free functions near `block_dominators`:

```rust
/// Direct and indirect call edges between module functions, for the
/// emission ordering. Recipes and naked bodies have no `Gen` run, so
/// they are neither sources nor targets: they never enter the exit-bank
/// map, and a caller must treat them as unknown exits.
fn call_edges<'a>(funcs: &[&'a Func]) -> HashMap<&'a str, Vec<&'a str>> {
    let known: std::collections::HashSet<&str> = funcs
        .iter()
        .filter(|f| !ir::is_runtime_routine(&f.name) && !f.naked)
        .map(|f| f.name.as_str())
        .collect();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for f in funcs {
        if !known.contains(f.name.as_str()) {
            continue;
        }
        for b in &f.blocks {
            for inst in &b.insts {
                let Inst::Call(c) = inst else { continue };
                let targets: Vec<&str> = if c.callees.is_empty() {
                    vec![c.func.as_str()]
                } else {
                    c.callees.iter().map(|s| s.as_str()).collect()
                };
                for t in targets {
                    if known.contains(t) {
                        edges.entry(f.name.as_str()).or_default().push(t);
                    }
                }
            }
        }
    }
    edges
}

/// Callees before callers, ties in module order. The call graph is a
/// DAG (recursion is a compile error upstream), so the scan always
/// progresses; the fallback branch is a defensive cycle break that
/// degrades to master's order and an absent map entry.
fn emission_order<'a>(funcs: &[&'a Func], edges: &HashMap<&str, Vec<&str>>) -> Vec<&'a Func> {
    let mut done: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut ordered = Vec::with_capacity(funcs.len());
    loop {
        let mut next = None;
        for f in funcs {
            if done.contains(f.name.as_str()) {
                continue;
            }
            let ready = edges
                .get(f.name.as_str())
                .map(|ts| ts.iter().all(|t| done.contains(t)))
                .unwrap_or(true);
            if ready {
                next = Some(f);
                break;
            }
        }
        match next {
            Some(f) => {
                done.insert(f.name.as_str());
                ordered.push(f);
            }
            None => {
                for f in funcs {
                    if !done.contains(f.name.as_str()) {
                        done.insert(f.name.as_str());
                        ordered.push(f);
                    }
                }
                return ordered;
            }
        }
    }
}
```

Split the single per-function loop into two passes over the same content.

Pass A (over `&order`) runs the `Gen` for every non-recipe, non-naked function and buffers instead of extending `out`/`locs`:

```rust
for f in &order {
    if ir::is_runtime_routine(&f.name) || f.naked {
        continue; // streamed by pass B, no Gen run, no map entry
    }
    // ... the existing ISR vector-glue check moves out; see pass B ...
    let mut g = Gen {
        /* unchanged fields, line ~6357 */
    };
    /* labels, phi_copies, preds, block_dominators, block loop: unchanged */
    g.flush_copies();
    bodies.insert(f.name.as_str(), (g.out, g.locs));
}
```

The `org 0x0008` push for a non-priority ISR (line ~6354) moves from pass A into pass B, so the vector line keeps its module-order position.

Pass B (over `funcs`, the original sorted order) streams everything that does not reorder: the recipe arm, the naked arm, and the ISR org line exactly as today, then pulls each remaining body from the buffer:

```rust
for f in funcs {
    // ... existing recipe arm, unchanged ...
    // ... existing naked arm, unchanged ...
    if f.isr && !priority_mode {
        out.push("    org 0x0008".to_string());
        locs.push(None);
    }
    let (lines, ls) = bodies
        .get(f.name.as_str())
        .expect("every non-recipe, non-naked function has a buffered body");
    out.extend(lines.clone());
    locs.extend(ls.iter().cloned());
}
```

Until Task 2 nothing writes an exit-bank map; this task only reorders emission and re-concatenates, so every module must assemble exactly as master did (apart from `tmp{n}` label numbering, which now follows the topological traversal).

- [ ] **Step 4: Verify behavior is unchanged**

Run: `make test CRATE=isel-pic18`
Expected: PASS, including `bodies_concatenate_in_module_order_even_when_emission_reorders` and every join/call test.

Run: `make check-warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(isel-pic18): buffer function bodies and emit callees first"
```

### Task 2: Exit-bank map and direct-call carry

**Files:**
- Modify: `crates/isel-pic18/src/lib.rs` (the per-function block loop's terminator recording, the `Inst::Call` arm at ~2845, the `Gen` struct at ~94, both `Gen` constructions)
- Test: `crates/isel-pic18/tests/isel_pic18.rs`

**Interfaces:**
- Consumes: Task 1's buffered emission and `bodies` map.
- Produces: `Gen.exit_banks: &'m HashMap<String, Option<u8>>`; `fn exit_bank(ends: &[Option<u8>]) -> Option<u8>`; the direct call arm sets `self.bsr` from the map instead of `None`.

- [ ] **Step 1: Write the failing carry tests**

Append to `crates/isel-pic18/tests/isel_pic18.rs`:

```rust
#[test]
fn a_provable_callee_exit_bank_carries_across_the_call() {
    // f ends on bank 2 on its only return path, so main's tracked bank
    // after `CALL f` is 2 and the bank-2 store re-selects nothing.
    // Without carry this emits a MOVLB after the call; with it, the whole
    // module has exactly the one MOVLB f's own body needs.
    let m = parse(
        "global a i8\nglobal b i8\nglobal c i8\nglobal d i8\nglobal g i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @a\n\
             %2 = load i8 @b\n\
             %3 = add i8 %1, %2\n\
             call void @f()\n\
             store i8 %3 @g\n\
             ret void\n\
         fn f(void) ()\n\
           block entry:\n\
             %1 = load i8 @c\n\
             %2 = load i8 @d\n\
             %3 = add i8 %1, %2\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("b", 0x21),
        ("c", 0x22),
        ("d", 0x23),
        ("g", 0x290),    // bank 2: the post-call store under test
        ("main::1", 0x30),
        ("main::2", 0x31),
        ("main::3", 0x32),
        ("f::1", 0x210), // bank 2
        ("f::2", 0x211), // bank 2
        ("f::3", 0x212), // bank 2: f's add dst selects the bank it exits on
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        1,
        "only f's own MOVLB; main's post-call store carries bank 2:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.ram_mut()[0x20] = 3;
    p.ram_mut()[0x21] = 4;
    p.run(500);
    assert!(p.halted());
    assert_eq!(p.ram()[0x212], 7, "f's add landed in f's slot");
    assert_eq!(p.ram()[0x290], 7, "main's store landed in bank 2 without re-selecting");
}

#[test]
fn a_terminator_selected_callee_exit_keeps_the_post_call_movlb() {
    // f's phi copies load from slots in different banks, one per edge,
    // so both predecessor terminators poison their end state, the exit
    // join is unknown, and main must still re-select after the call.
    // (A mid-block bank select would NOT do this: it is cleared before
    // the terminator lowering and leaves the end state provable.)
    let m = parse(
        "global c i8\nglobal q1 i8\nglobal q2 i8\nglobal g i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f()\n\
             store i8 7 @g\n\
             ret void\n\
         fn f(void) ()\n\
           block entry:\n\
             %1 = load i8 @c\n\
             br i1 %1 t u\n\
           block t:\n\
             br merge\n\
           block u:\n\
             br merge\n\
           block merge:\n\
             %2 = phi i8 [%q1, t], [%q2, u]\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("c", 0x20),
        ("q1", 0x110), // bank 1: the t-edge copy selects it
        ("q2", 0x290), // bank 2: the u-edge copy selects it
        ("g", 0x090),  // bank 0: the post-call store must re-select it
        ("f::1", 0x30),
        ("f::2", 0x31), // the phi merge slot
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        3,
        "t-edge bank 1, u-edge bank 2, and main's re-select; nothing carries:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for c in [1u8, 0] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        p.ram_mut()[0x20] = c;
        p.run(500);
        assert!(p.halted());
        assert_eq!(p.ram()[0x090], 7, "main's store lost on cond={c}");
    }
}

#[test]
fn disagreeing_callee_returns_keep_the_post_call_movlb() {
    // f's two arms return on different banks, so the exit join is
    // unknown and main re-selects. Both arms run in the simulator.
    let m = parse(
        "global a i8\nglobal c i8\nglobal g i8\nglobal k i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i8 @a\n\
             call void @f()\n\
             store i8 %1 @g\n\
             ret void\n\
         fn f(void) ()\n\
           block entry:\n\
             %1 = load i8 @c\n\
             br i1 %1 t u\n\
           block t:\n\
             store i8 1 @k\n\
             ret void\n\
           block u:\n\
             store i8 2 @slot\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("c", 0x21),
        ("g", 0x090),    // bank 0: main must re-select it after the call
        ("k", 0x110),    // bank 1: arm t exits here
        ("slot", 0x290), // bank 2: arm u exits here
        ("main::1", 0x30),
        ("f::1", 0x31),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        block_section(&asm, "main").contains("MOVLB 0x0"),
        "main must re-select bank 0 after the unknown exit:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    for (c, want_k, want_slot) in [(1u8, 1u8, 0u8), (0u8, 0u8, 2u8)] {
        let mut p = pic14_sim::Pic18::new(words.clone());
        p.ram_mut()[0x20] = c;
        p.run(500);
        assert!(p.halted());
        assert_eq!(p.ram()[0x090], c, "main's store lost on cond={c}");
        assert_eq!(p.ram()[0x110], want_k, "arm t's store wrong on cond={c}");
        assert_eq!(p.ram()[0x290], want_slot, "arm u's store wrong on cond={c}");
    }
}

#[test]
fn a_forward_defined_callee_with_provable_exit_still_carries() {
    // helper is defined after main. The map is filled by the buffered
    // reverse-topological emission, so the textual order must not matter.
    let m = parse(
        "global a i8\nglobal g i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @helper()\n\
             store i8 7 @g\n\
             ret void\n\
         fn helper(void) ()\n\
           block entry:\n\
             store i8 1 @g\n\
             ret void\n",
    );
    let addrs = addrs(&[("a", 0x20), ("g", 0x090)]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        1,
        "helper selects bank 0; main's store carries it:\n{asm}"
    );
    let words = asm::assemble_pic18(&asm);
    let mut p = pic14_sim::Pic18::new(words);
    p.run(500);
    assert!(p.halted());
    assert_eq!(p.ram()[0x090], 7);
}

#[test]
fn a_recipe_callee_keeps_the_post_call_movlb() {
    // __mul_u16 has no Gen run: its body streams from the recipe in the
    // concat loop and its exit never enters the map, so main must
    // re-select after the call. The stub's alloca-only entry block must
    // not leak into the output as an empty label either.
    let m = parse(
        "global a i16\nglobal g i8\n\
         fn __mul_u16(i16) (val=i16)\n\
           block entry:\n\
             %__scr = alloca 8\n\
         fn main(void) ()\n\
           block entry:\n\
             %1 = load i16 @a\n\
             %2 = call i16 @__mul_u16(i16 %1)\n\
             store i8 7 @g\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("g", 0x090),
        ("main::1", 0x30),
        ("main::2", 0x32),
        ("__mul_u16::val", 0x40),
        ("__mul_u16::__scr", 0x50),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(asm.contains("    CALL __mul_u16"), "recipe call:\n{asm}");
    assert!(
        block_section(&asm, "main").contains("MOVLB 0x0"),
        "main must re-select after the recipe call:\n{asm}"
    );
}
```

The behavioral side of recipe calls is already pinned by the existing
`muldiv` e2e (full pipeline through legalize's real routine calls, run
in the simulator); this task's Step 4 runs it.

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `make exec CMD='cargo test -p isel-pic18 --test isel_pic18 provable_callee_exit terminator_selected_callee_exit disagreeing_callee_returns forward_defined_callee recipe_callee -- --test-threads=1'`
Expected: `a_provable_callee_exit_bank_carries_across_the_call` and `a_forward_defined_callee_with_provable_exit_still_carries` FAIL on the MOVLB count (2 today, 1 wanted); `a_terminator_selected_callee_exit_keeps_the_post_call_movlb`, `disagreeing_callee_returns_keep_the_post_call_movlb`, and `a_recipe_callee_keeps_the_post_call_movlb` PASS already (they pin today's conservative behavior and must keep passing).

- [ ] **Step 3: Implement the map and the carry**

In the `Gen` struct (after `bsr_dirty`, line ~114):

```rust
/// Callee name -> the bank every return path leaves live, from the
/// buffered reverse-topological emission. A caller relies on an entry
/// only after the map proved it from the code as actually emitted;
/// absent or `None` entries clear the tracked bank exactly as master.
exit_banks: &'m HashMap<String, Option<u8>>,
```

Add `exit_banks: &exits` to both `Gen` constructions (recipe path ~6261 and ordinary path ~6357).

In `select_with_locs`, next to Task 1's `bodies` map:

```rust
let mut exits: HashMap<String, Option<u8>> = HashMap::new();
```

In pass A's per-function loop, collect return ends. Add before the block loop:

```rust
let mut ret_ends: Vec<Option<u8>> = Vec::new();
```

After the existing `block_end.insert(b.label.clone(), end)` (line ~6940):

```rust
if matches!(b.insts.last(), Some(Inst::Ret(..))) {
    ret_ends.push(end);
}
```

After the block loop, before Task 1's `bodies.insert(...)`:

```rust
exits.insert(f.name.to_string(), exit_bank(&ret_ends));
```

Add the free function next to `emission_order`:

```rust
/// The bank a function leaves live on return: the join over its
/// RETURN-ending blocks' recorded end states. Unanimous known ends
/// give the bank; any dirty, missing, or disagreeing end gives
/// unknown.
fn exit_bank(ends: &[Option<u8>]) -> Option<u8> {
    let mut known: Option<u8> = None;
    for e in ends {
        match (known, e) {
            (_, None) => return None,
            (None, Some(v)) => known = Some(*v),
            (Some(v), Some(w)) if v == w => {}
            (Some(_), Some(_)) => return None,
        }
    }
    known
}
```

Rewrite the direct call arm's invalidation (lines ~2846-2855). Replace the comment and the `self.bsr = None;` with:

```rust
                // A `CALL` return joins like a label: the callee ran its
                // own `MOVLB` sequence. With the exit-bank map the join
                // is precise: a callee whose every return path ends at
                // one bank leaves that bank live, so the tracked value
                // carries; an unknown exit clears. FSR0 has no such
                // contract: the callee used it for its own pointer
                // accesses (epic-cc#472). (epic-cc#495)
                self.bsr = self.exit_banks.get(&c.func).copied().flatten();
                self.fsr0_holds = None;
```

Add an in-file unit test near the existing `operand()` unit tests (~7180):

```rust
#[test]
fn exit_bank_joins_return_ends() {
    assert_eq!(exit_bank(&[Some(2), Some(2)]), Some(2));
    assert_eq!(exit_bank(&[Some(1), Some(2)]), None);
    assert_eq!(exit_bank(&[None]), None);
    assert_eq!(exit_bank(&[]), None);
}
```

- [ ] **Step 4: Run the full crate suite**

Run: `make test CRATE=isel-pic18`
Expected: PASS, including the three conservative tests and the pre-existing `call_return_invalidates_tracked_bsr_so_a_later_banked_access_is_not_misbanked` (its callee exits on bank 2 and its post-call access is bank 1, so the carry still forces the re-bank; if it fails, the carry regressed, do not weaken the test).

Run the recipe-call e2e for the behavioral side:

Run: `make exec CMD='cargo test -p isel-pic18 --test e2e muldiv'`
Expected: PASS.

Run: `make check-warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "perf(isel-pic18): carry provable callee exit banks across calls"
```

### Task 3: Indirect-call candidate carry

**Files:**
- Modify: `crates/isel-pic18/src/lib.rs` (the `emit_indirect_call` candidate loop, line ~2963)
- Test: `crates/isel-pic18/tests/isel_pic18.rs`

**Interfaces:**
- Consumes: Task 2's `Gen.exit_banks` and `exit_bank`.
- Produces: per-candidate `CALL` arms set `self.bsr` from the map, and the shared done label's tracked bank is restored to the candidate meet directly after `emit_label`, because the label's linear fall-through comes from the trap block (bank unknown) and the forward join cannot restore.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn indirect_candidates_with_unanimous_exit_carry_at_the_join() {
    // Both candidates end on bank 1, so the done label's true entry bank
    // is 1 (only the candidate arms' BRAs reach it; the trap loops). The
    // meet is restored directly after the label: the forward join cannot
    // do it, because the label's linear fall-through comes from the trap
    // block, whose bank is unknown. Without carry this emits a MOVLB in
    // main after the chain; with it, exactly the candidates' two.
    let m = parse(
        "global a i8\nglobal g i8\n\
         fn f0(void) ()\n\
           block entry:\n\
             store i8 1 @h\n\
             ret void\n\
         fn f1(void) ()\n\
           block entry:\n\
             store i8 2 @h\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             call void %3() callees f0 f1\n\
             store i8 9 @g\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("g", 0x110),  // bank 1: the post-chain store under test
        ("h", 0x190),  // bank 1: both candidates select it and exit on it
        ("main::3", 0x30),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert_eq!(
        asm.matches("MOVLB").count(),
        2,
        "one MOVLB per candidate body, none in main:\n{asm}"
    );
}

#[test]
fn mixed_indirect_candidates_clear_at_the_join() {
    // f1 exits on bank 2, f0 on bank 1: the join must collapse and the
    // post-chain store must re-select bank 1.
    let m = parse(
        "global a i8\nglobal g i8\n\
         fn f0(void) ()\n\
           block entry:\n\
             store i8 1 @h\n\
             ret void\n\
         fn f1(void) ()\n\
           block entry:\n\
             store i8 2 @k\n\
             ret void\n\
         fn main(void) ()\n\
           block entry:\n\
             call void %3() callees f0 f1\n\
             store i8 9 @g\n\
             ret void\n",
    );
    let addrs = addrs(&[
        ("a", 0x20),
        ("g", 0x110), // bank 1
        ("h", 0x190), // bank 1: f0's exit
        ("k", 0x290), // bank 2: f1's exit
        ("main::3", 0x30),
    ]);
    let asm = select(&PIC18F4550, &m, &addrs, None);
    assert!(
        block_section(&asm, "main").contains("MOVLB 0x1"),
        "main must re-select bank 1 after the mixed-exit chain:\n{asm}"
    );
}
```

- [ ] **Step 2: Run them to verify the first fails**

Run: `make exec CMD='cargo test -p isel-pic18 --test isel_pic18 indirect_candidates mixed_indirect_candidates'`
Expected: `indirect_candidates_with_unanimous_exit_carry_at_the_join` FAIL (3 MOVLBs today: two candidate bodies plus main's re-select); `mixed_indirect_candidates_clear_at_the_join` PASS (pins conservatism).

- [ ] **Step 3: Implement the per-candidate carry and the done-label restore**

In `emit_indirect_call`, compute the candidate exits up front, use them per candidate, and restore the meet after the done label. Replace lines ~2950 and ~2963-2965:

```rust
    let l_done = self.fresh_label();
    let cand_exits: Vec<Option<u8>> = callees
        .iter()
        .map(|cand| self.exit_banks.get(cand).copied().flatten())
        .collect();
    for (cand, exit) in callees.iter().zip(cand_exits.iter()) {
```

and inside the matched arm:

```rust
            self.emit(format!("    CALL {cand}"));
            // Same contract as the direct arm, per candidate: a proven
            // exit bank carries, an unknown one clears.
            self.bsr = *exit;
            self.fsr0_holds = None;
```

Then after the shared label, before the retval copy:

```rust
        self.emit_label(&l_done);
        // Only the candidate arms' BRAs reach the done label (the trap
        // loops), so the candidate exit meet is the label's true entry
        // bank. Restore it directly: the forward join cannot, because
        // the label's linear fall-through comes from the trap block,
        // whose bank is unknown. (epic-cc#495)
        if let Some(bank) = exit_bank(&cand_exits) {
            self.bsr = Some(bank);
        }
```

- [ ] **Step 4: Run the full crate suite**

Run: `make test CRATE=isel-pic18`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "perf(isel-pic18): carry exit banks through indirect call candidates"
```

### Task 4: Re-profile, re-baseline, ADR

**Files:**
- Modify: `crates/driver/tests/fixtures/size_baseline.toml` (menu-demo and any shrunk rows)
- Create: `docs/adr/ADR-039-pic18-exit-bank-carry.md`
- Modify: `docs/03-decisions.md` (one index line)
- Delete: `docs/superpowers/plans/2026-09-22-pic18-exit-bank-carry.md` (final commit only)

**Interfaces:**
- Consumes: the completed Tasks 1-3.
- Produces: the landed change with measured acceptance and the distilled decision record.

- [ ] **Step 1: Re-run the gate script and the profile**

Re-run Task 0's Steps 2-3 in the worktree after the change. The gate script's provable-site count measures opportunity and stays roughly constant; the acceptance signal is the MOVLB total, which should drop by roughly Task 0's provable-site count. Run `scripts/density-profile.py target/menu-demo.asm` for the sink table.

- [ ] **Step 2: Run the full suite**

Run: `make test`
Expected: PASS everywhere except possibly `size_regression_e2e`, which fails on the deliberately improved menu-demo row.

- [ ] **Step 3: Re-baseline the ladder deliberately**

Run: `make exec CMD='UPDATE_SIZE_BASELINE=1 cargo test -p driver --test size_regression_e2e'`
Then inspect: `git diff crates/driver/tests/fixtures/size_baseline.toml`.
Expected: rows shrink or hold; the menu-demo row drops by the measured amount. Any growing row is a bug: STOP and investigate before committing.

- [ ] **Step 4: Write the ADR and index line**

Create `docs/adr/ADR-039-pic18-exit-bank-carry.md` with a Status line (accepted, 2026-09-22, epic-cc#495), the decision (buffered reverse-topological emission, exit-bank map from recorded return ends, direct and indirect call arms carry provable exits, unknown clears), the rationale (emitter-truth: the map is derived from the same run's recorded end states, so it cannot disagree with the emitted text; no return-site tax), and the rejected alternatives (the restore-on-return convention, rejected on docs/42's measurement; a PIC18 banking-pass port, rejected as a duplicate tracking system). Add one line to `docs/03-decisions.md`.

- [ ] **Step 5: Commit**

```bash
git add docs/adr/ADR-039-pic18-exit-bank-carry.md docs/03-decisions.md crates/driver/tests/fixtures/size_baseline.toml
git commit -m "docs: ADR-039 pic18 exit-bank carry"
```

- [ ] **Step 6: File the deferrals and report on the ticket**

Before the PR, file any findings the work surfaced but did not fix as issues and card them with `epic-tasks add` (at minimum: exit banks for runtime recipes, if float-heavy call sites left residual MOVLBs). Comment the before/after MOVLB counts on epic-cc#495.

### Task 5: Review gate, takeoff, PR

**Files:**
- None new; process only.

**Interfaces:**
- Consumes: the finished branch `perf/495-exit-bank-carry`.
- Produces: an approved review, a clean takeoff, and the pull request.

- [ ] **Step 1: Dispatch a separate reviewer**

Dispatch a reviewer agent (not the author) over the branch diff with a stated time budget (expectation and a hard limit at least 3x, never below 30 minutes), the repo's conventions to check (build, tests, prose, docs), and the instruction that a wrap-up signal may arrive and it must finalize immediately, flagging anything unfinished as PARTIAL.

- [ ] **Step 2: Address findings**

Fix what the review found, re-run the affected checks (`make test CRATE=isel-pic18`, `make check-warnings`), and delete the plan file in the final commit:

```bash
git rm docs/superpowers/plans/2026-09-22-pic18-exit-bank-carry.md
git commit -m "chore: remove the exit-bank carry implementation plan"
```

- [ ] **Step 3: Takeoff ritual**

Run: `make pre-pr-check TEST=1`
Expected: exit 0 (worktree, clean tree, commit hygiene, warnings, prose, PR-body readiness, hooks).

- [ ] **Step 4: Push and open the PR with real newlines**

```bash
git push -u origin perf/495-exit-bank-carry
cat <<'EOF' > /tmp/pr_body.md
Closes #495

Carry each callee's provable BSR exit bank into the caller instead of
clearing the tracked bank at every call return.

- functions emit into per-function buffers in reverse topological call
  order, concatenated in module order
- a function's exit bank is the join over its RETURN-ending block end
  states, recorded by the same emission that produced the text
- direct calls and indirect call candidates set the tracked bank from
  the exit map; unknown exits clear exactly as before
- before/after menu-demo MOVLB counts: <fill from Task 4>

ADR-039 records the decision; the restore-on-return convention stays
rejected per docs/42.
EOF
gh pr create --title "perf(isel-pic18): interprocedural BSR exit-bank carry" --body-file /tmp/pr_body.md
```

Replace `<fill from Task 4>` with the measured counts. Then record it:

```bash
epic-tasks review epic-cc#495 --pr <url>
```
