# Integer Spine — Milestone 2: Control Flow, Calls, and 16-bit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Compile the probe (loop + `if` + function call + 16-bit arithmetic) end-to-end through the ten-stage pipeline to Intel HEX that runs correctly in `pic14-sim` — `out == 48` for `in == 5` — cross-checked against `gpasm`.

**Architecture:** Extends the milestone-1 crates (all on master). `crates/ir` gains the control-flow/call/cast instruction variants and their canonical text serialization; `irparse` parses them from `.ll`; `callgraph` builds real edges and detects recursion/depth; `isel` ports the verified spike codegen (16-bit carry chains, phi elimination, select/icmp, calls via arg/ret slots); `driver` needs no change. Allocation stays function-scoped (the spike's proven approach) — overlay allocation and real banking are milestone 3.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

**Spec:** [`docs/12-backend-design.md`](../12-backend-design.md) §4 phase 2.

## Global Constraints

- Build/test with `nix develop --command cargo …`; never `apt install` toolchain deps.
- clang driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (`-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc`).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler in the product; `gpasm` external-process test-only; GPL never linked.
- Text boundaries: stages communicate via text; the `ir` crate defines the IR text format. The driver may import stage libraries; stages do not import each other.
- New files must be `git add`ed before `nix develop` sees them.
- Unsupported constructs panic loudly, never silently miscompile.

## The verified reference

The throwaway spike (gitignored, on disk) already compiled this exact probe and ran it (`out == 48`): `spike/src/ir.rs` (parser), `spike/src/codegen.rs` (allocation, phi elimination, isel), `spike/probe.ll` (the target IR). Milestone 2 ports that proven codegen into the milestone-1 crate architecture. Every isel task below names the spike function to port and the `.asm` pattern it must emit; the spike is the ground truth for the patterns.

## Probe `.ll` (at `-O1`) — the acceptance input

`spike/probe.ll` (already on disk). Function `main`: `load volatile @in` → `zext i8→i16` → `icmp eq i8, 0` → `br i1` → loop block with two `phi i16`, `and i16`, `icmp eq i16`, `select i1`, `call i16 @add`, `add i16` (inc), `icmp eq i16`, `br i1` → exit block `trunc i16→i8`, `phi i8`, `store volatile @out`, `ret void`. Function `add(i16, i16) -> i16`: `add nsw i16`, `ret i16`. Expected: `in = 5` → `out == 48`.

## IR text format additions (`crates/ir`)

New instruction lines (append to the milestone-1 format):

```
%d = zext <ty> %v to <ty>             ; zext i8 %1 to i16
%d = trunc <ty> %v to <ty>            ; trunc i16 %14 to i8
%d = icmp <pred> <ty> <a> <b>         ; pred ∈ eq,ne (panic on others for now)
%d = select i1 <c> <ty> <a> <b>
br <label>                             ; unconditional
br i1 <cond> <t> <f>                   ; conditional
%d = call <ty> @<fn>(<ty> <val>, ...)  ; valued call
call void @<fn>(...)                   ; void call
%d = phi <ty> <val> <pred> <val> <pred> ...  ; pairs of (incoming val, pred block label)
```

Block labels are the `.ll` labels as-is (`0`, `4`, `6`, `8`; first block label is the entry). `phi` preds reference block labels.

---

### Task 1: `ir` — control-flow/call/cast instruction variants

**Files:**
- Modify: `crates/ir/src/lib.rs`
- Test: `crates/ir/tests/roundtrip.rs` (extend)

**Interfaces:**
- Consumes: existing `ir` types + text format.
- Produces: new `Inst` variants `Zext{dst,from,val,to}`, `Trunc{dst,from,val,to}`, `Icmp{dst,pred,ty,a,b}`, `Select{dst,cond,ty,a,b}`, `Call{dst:Option<String>, ty:Option<Ty>, func, args:Vec<(Ty,Val)>}`, `Br{target}`, `BrCond{cond,t,f}`, `Phi{dst,ty,incoming:Vec<(Val,String)>}` — with `serialize`/`parse` for each, in the canonical line forms above.

- [ ] **Step 1: Extend the failing test** — append to `crates/ir/tests/roundtrip.rs` a module exercising every new variant and assert the round-trip fixed point (parse → serialize → parse → serialize is stable) and that key canonical lines appear (e.g. `%9 = phi i16 0 main %15 main_L8`, `br i1 %3 6 8`, `%14 = call i16 @add(i16 %10, i16 %13)`, `call void @f()`, `%2 = zext i8 %1 to i16`, `%5 = trunc i16 %14 to i8`, `%12 = icmp eq i16 %11 0`, `%13 = select i1 %12 i16 100 i16 %9`). Use exactly the canonical syntax above (space-separated, no commas in the serialized form).
- [ ] **Step 2: Run to verify it fails** — `nix develop --command cargo test -p ir`.
- [ ] **Step 3: Implement** — add the variants to `Inst`, the `inst_str`/`parse_inst` arms, and a `Call` arg list serializer/parser. `phi` serializes as `%d = phi <ty> <v1> <p1> <v2> <p2> …`; parse splits into `(val, pred)` pairs. `icmp` pred is a bare word (`eq`/`ne`); parse rejects others loudly. `Call` serializes args as `(<ty> <val>, …)` with no spaces inside parens; `call void @f()` has no dst.
- [ ] **Step 4: Run to verify it passes** — `nix develop --command cargo test -p ir`.
- [ ] **Step 5: Commit** — `git add crates/ir && git commit -m "feat(ir): control-flow, call, and cast instruction variants"`.

---

### Task 2: `irparse` — parse the new `.ll` opcodes

**Files:**
- Modify: `crates/irparse/src/lib.rs`
- Test: `crates/irparse/tests/parse_ll.rs` (extend)

**Interfaces:**
- Consumes: `ir` new variants (Task 1).
- Produces: `parse_ll` handles `zext`/`trunc`/`icmp`/`select`/`br`/`call`/`phi` from `.ll` text, stripping attributes (`nsw`, `nuw`, `nneg`, `tail`, `fastcc`, `noundef`, `align`, metadata) — port the corresponding `parse_inst` arms from `spike/src/ir.rs` (on disk), adapting to the `ir` struct-form variants and the canonical text syntax. `call` args keep their order; `phi` incoming pairs map `(val, pred-label)`.

- [ ] **Step 1: Extend the failing test** — append a test that parses the full probe `.ll` (`spike/probe.ll`, read via `include_str!` is not possible cross-crate — copy the IR text into the test, or read the file at runtime with `std::fs` and a relative path; prefer embedding a trimmed copy) and asserts: 2 functions, block labels `0/4/6/8` in main, a `Phi` with the correct incoming pairs, a `Call` to `@add` with 2 args, and a `BrCond`.
- [ ] **Step 2: Run to verify it fails** — `nix develop --command cargo test -p irparse`.
- [ ] **Step 3: Implement** — port the arms. `br label %6` → `Br{6}`; `br i1 %3, label %6, label %8` → `BrCond{%3, 6, 8}`; `call i16 @add(i16 %10, i16 %13)` → `Call{Some(14), I16, "add", [(I16, Reg 10), (I16, Reg 13)]}`; `%7 = phi i8 [ 0, %0 ], [ %5, %4 ]` → `Phi{7, I8, [(Const 0, "0"), (Reg 5, "4")]}`.
- [ ] **Step 4: Run to verify it passes** — `nix develop --command cargo test -p irparse`.
- [ ] **Step 5: Commit** — `git commit -m "feat(irparse): parse control flow, calls, and casts"`.

---

### Task 3: `callgraph` — real edges, recursion, depth

**Files:**
- Modify: `crates/callgraph/src/lib.rs`
- Test: `crates/callgraph/tests/graph.rs` (extend)

**Interfaces:**
- Consumes: `ir::Inst::Call`.
- Produces: `build(&Module)` walks every block, adds `(caller, callee)` for each `Call`; `max_depth` = longest call-chain path (1 for no calls); `check_depth(g, limit)` panics if `max_depth > limit`; **recursion detection**: panic with a clear message if the call graph has a cycle (DFS-based).

- [ ] **Step 1: Extend the failing test** — (a) `main → add` yields edge `("main","add")` and depth 2; (b) `f → g → f` panics with a recursion message; (c) `f → g → h` depth 3 passes `check_depth(8)` and panics on `check_depth(2)`.
- [ ] **Step 2: Run to verify it fails** — `nix develop --command cargo test -p callgraph`.
- [ ] **Step 3: Implement** — DFS over the call graph: detect back edges (cycle → panic "recursion"), compute longest path depth. The IR text parse must support the `call` line for the test inputs (use `ir::parse` with the canonical form from Task 1).
- [ ] **Step 4: Run to verify it passes** — `nix develop --command cargo test -p callgraph`.
- [ ] **Step 5: Commit** — `git commit -m "feat(callgraph): real edges, recursion detection, depth check"`.

---

### Task 4: `alloc` — i16 global sizing

**Files:**
- Modify: `crates/alloc/src/lib.rs`
- Test: `crates/alloc/tests/alloc.rs` (extend)

**Interfaces:**
- Consumes: `ir::Global { ty }`.
- Produces: `allocate` advances addresses by `ty.bytes()` (i8=1, i16=2) for non-const globals (closes the milestone-1 deferred minor).

- [ ] **Step 1: Extend the failing test** — `global a i8` then `global b i16` → `a.addr == 0x20`, `b.addr == 0x22`.
- [ ] **Step 2: Run to verify it fails**.
- [ ] **Step 3: Implement** — replace `addr += 1` with `addr += g.ty.bytes()`.
- [ ] **Step 4: Run to verify it passes**.
- [ ] **Step 5: Commit** — `git commit -m "fix(alloc): size global addresses by type width"`.

---

### Task 5: `isel` — 16-bit arithmetic, casts, and phi elimination

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Consumes: `ir` new variants; the address map (`global <name> 0xNN`); block labels in IR text.
- Produces: 16-bit binop lowering, `zext`/`trunc`, and phi-elimination copies. Port from `spike/src/codegen.rs`: `emit_add16` (reg+reg and reg+const carry chains), `emit_and16`, the `Zext`/`Trunc` arms, and the phi-copies mechanism (emit `MOVF`/`MOVLW` copies into the phi destination slot at the end of each predecessor block, before the terminator). Per-function slot maps keyed by `{func}::{name}` (already the milestone-1 pattern) — keep it.

**Emitted patterns (the contract):**
- `add i16 a b` (reg+reg): `MOVF b_lo,W; ADDWF a_lo,W; MOVWF d_lo; MOVF b_hi,W; BTFSC STATUS,0; ADDLW 1; ADDWF a_hi,W; MOVWF d_hi`
- `add i16 a k` (reg+const): `MOVF a_lo,W; ADDLW k_lo; MOVWF d_lo; MOVF a_hi,W; BTFSC STATUS,0; ADDLW 1; ADDLW k_hi; MOVWF d_hi`
- `and i16 a k` (reg+const): `MOVF a_lo,W; ANDLW k_lo; MOVWF d_lo; MOVF a_hi,W; ANDLW k_hi; MOVWF d_hi`
- `zext i8 v to i16`: copy `v` to `d_lo`, `CLRF d_hi`
- `trunc i16 v to i8`: copy `v_lo` to `d`
- 16-bit values occupy two consecutive slots (lo at `addr`, hi at `addr+1`); slot allocation must reserve 2 bytes for i16.

- [ ] **Step 1: Extend the failing test** — a module with `%a = add i16 %x %y` where `%x`/`%y` are loaded i16s from globals, asserting the carry-chain lines appear; a `zext`/`trunc` pair; a two-block program with a phi and assert the copy lands before the terminator of each predecessor.
- [ ] **Step 2: Run to verify it fails**.
- [ ] **Step 3: Implement** — port the spike functions. The phi copies: for each block, collect `Phi` instructions, build `(dst, ty, val)` copy entries per predecessor label, emit them before the block terminator (matching the spike's `phi_copies` map).
- [ ] **Step 4: Run to verify it passes**.
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): 16-bit arithmetic, casts, and phi elimination"`.

---

### Task 6: `isel` — control flow (br, brcond, icmp, select)

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Consumes: block label map (entry → `{fn}`, others → `{fn}_L{label}` — the milestone-1/spike scheme).
- Produces: `Br{target}` → `GOTO <label>`; `BrCond{cond,t,f}` → `MOVF cond,W; BTFSC STATUS,2; GOTO <f>; GOTO <t>` (port `emit_cond_branch`); `Icmp{eq}` → XOR-based Z-flag compare + materialize i1 (port `emit_cmp_eq`; 16-bit via `IORWF scratch` accumulation); `Select` → branch-based (port `emit_select`).

**Emitted patterns (the contract):**
- `icmp eq i8 a b` → `MOVF a,W; XORWF b,W; MOVWF scratch; MOVLW 0; BTFSC STATUS,2; MOVLW 1; MOVWF d`
- `icmp eq i16 a b` → byte 0 XOR into scratch, byte 1 XOR + `IORWF scratch,W` accumulation, then the i1 materialize
- `select i1 c ty a b` → `MOVF c,W; BTFSC STATUS,2; GOTO L_else; <copy a to d>; GOTO L_end; L_else: <copy b to d>; L_end:`
- `br i1 c t f` → `MOVF c,W; BTFSC STATUS,2; GOTO <f>; GOTO <t>`
- `br t` → `GOTO <t>`
- A scratch byte (fixed at 0x2A, after globals — or allocated) is needed for the i16 icmp; the driver's globals in/out are 0x20/0x21.

- [ ] **Step 1: Extend the failing test** — a module with a conditional branch on an icmp result and a select, asserting the skip/branch lines.
- [ ] **Step 2: Run to verify it fails**.
- [ ] **Step 3: Implement** — port the spike functions.
- [ ] **Step 4: Run to verify it passes**.
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): branches, compares, and select"`.

---

### Task 7: `isel` — calls and returns

**Files:**
- Modify: `crates/isel/src/lib.rs`
- Test: `crates/isel/tests/isel.rs` (extend)

**Interfaces:**
- Consumes: `ir::Inst::Call`, function table (callee params).
- Produces: `Call{dst, ty, func, args}` → copy each arg into the callee's param slots (`MOVF arg,W; MOVWF callee_param_slot`), `CALL func`, then copy `RETVAL_LO/HI` into `dst`; `Ret(Some((ty, val)))` → copy `val` into `RETVAL_LO/HI`, `RETURN`. Port `emit_call`/the `Ret` arm from the spike. RETVAL slots are 2 bytes immediately after the address map globals (milestone 1 reserved this pattern; allocate retval_lo/hi after globals, e.g. 0x23/0x24 for the probe's two i8 globals). Callee param slots are the callee's function-scoped slots.

**Emitted pattern (the contract):**
- `%14 = call i16 @add(i16 %10, i16 %13)` → `MOVF %10_lo,W; MOVWF add::0_lo; MOVF %10_hi,W; MOVWF add::0_hi; MOVF %13_lo,W; MOVWF add::1_lo; …; CALL add; MOVF retval_lo,W; MOVWF %14_lo; MOVF retval_hi,W; MOVWF %14_hi`
- `ret i16 %3` → `MOVF %3_lo,W; MOVWF retval_lo; MOVF %3_hi,W; MOVWF retval_hi; RETURN`

- [ ] **Step 1: Extend the failing test** — a module with a valued call to a function with two i16 params, asserting arg-copy, `CALL`, and retval-copy lines; a `ret i16` test asserting retval writes + `RETURN`.
- [ ] **Step 2: Run to verify it fails**.
- [ ] **Step 3: Implement** — port. The callee's param slots must be allocated in the callee's slot map (function-scoped; allocate callee params when emitting the callee, and look them up cross-function — the milestone-1 isel already has per-function maps; extend with a `ssa_addr_in(func, name)` cross-function lookup).
- [ ] **Step 4: Run to verify it passes**.
- [ ] **Step 5: Commit** — `git commit -m "feat(isel): calls and returns with arg/ret slots"`.

---

### Task 8: End-to-end — the probe through the driver + gpasm cross-check

**Files:**
- Create: `crates/driver/tests/fixtures/probe.c`, `crates/driver/tests/probe_e2e.rs`
- Create: `crates/asm/tests/fixtures/probe.asm`, `crates/asm/tests/gpasm_probe.rs`
- Modify: `crates/driver/src/main.rs` (only if a stage interface changed — otherwise untouched)

**Interfaces:**
- Consumes: all stage crates; `pic14_sim`.

- [ ] **Step 1: Write the failing probe e2e test**

`crates/driver/tests/fixtures/probe.c` (the spike probe):

```c
volatile unsigned char in;
volatile unsigned char out;
__attribute__((noinline)) static int add(int a, int b) { return a + b; }
void main(void) {
    int n = in;
    int t = 0;
    for (int i = 0; i < n; i++) {
        if (i & 1) t = add(t, i);
        else      t = add(t, 100);
    }
    out = (unsigned char)t;
}
```

`crates/driver/tests/probe_e2e.rs`:

```rust
use std::process::Command;
#[test]
fn probe_runs_correctly() {
    let out = Command::new(env!("CARGO_BIN_EXE_driver"))
        .args(["tests/fixtures/probe.c", "tests/fixtures/probe.hex"])
        .output().expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));
    let hex = std::fs::read_to_string("tests/fixtures/probe.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 5; // in = 5
    p.run(200_000);
    assert_eq!(p.ram()[0x21], 48); // out == 48
    assert!(p.halted());
}
```

- [ ] **Step 2: Run to verify it fails** — `nix develop --command cargo test -p driver --test probe_e2e`. (Debug any stage gaps — likely isel panic messages; fix in the isel tasks' files, not the test.)
- [ ] **Step 3: Write the gpasm cross-check for the probe's asm**

`crates/asm/tests/fixtures/probe.asm` — take the `.asm` the driver produced for `probe.c` (run the driver manually once it passes, copy `probe.asm` output into the fixture; regenerate it with `cargo run -p driver -- crates/driver/tests/fixtures/probe.c /tmp/probe.hex` and extract the asm — the driver currently writes only hex, so run the stage binaries manually: `irparse`→`wholeprog`→`legalize`→`callgraph`→`alloc`→`isel`→`banking`→`peephole`→ capture the final `.asm`). Then `crates/asm/tests/gpasm_probe.rs` asserts our `assemble_file_to_hex` output equals gpasm's HEX byte-for-byte (mirror `crates/asm/tests/gpasm_cross.rs` from milestone 1) and runs it in `pic14_sim` asserting `out == 48`.
- [ ] **Step 4: Verify the full suite** — `nix develop --command cargo test` (all crates green).
- [ ] **Step 5: Commit** — `git add crates/driver/tests crates/asm/tests && git commit -m "test(e2e): probe compiles, assembles, and runs correctly"`.

---

## Self-review notes

- **Spec coverage:** milestone 2 delivers the probe (the milestone-1 plan's declared next step — control flow, calls, 16-bit) end-to-end, per spec §4 phase 2. Overlay allocation and real banking are explicitly milestone 3 (the spike proved function-scoped allocation sufficient for the probe; overlay is an optimization the probe does not exercise).
- **Deferred (later milestones, panics loudly until then):** `icmp` predicates beyond `eq`; `gep`/pointers (phase 3); constant folding (both-const binops panic); multi-bank `BANKSEL` (milestone 3).
- **Type consistency:** `ir` variants are struct-form like the existing ones (`Inst::Call{dst,ty,func,args}`, `Inst::Br{target}`, `Inst::BrCond{cond,t,f}`, `Inst::Phi{dst,ty,incoming}`, `Inst::Icmp{dst,pred,ty,a,b}`, `Inst::Select{dst,cond,ty,a,b}`, `Inst::Zext{dst,from,val,to}`, `Inst::Trunc{dst,from,val,to}`). The canonical text forms in the format section are the contract between `ir` serialize and every consumer.
- **The spike (`spike/src/ir.rs`, `spike/src/codegen.rs`, on disk) is the verified reference** for the isel patterns and the parser arms — port, don't redesign. The spike's probe already produced `out == 48`; this plan reproduces that result through the product crates.
