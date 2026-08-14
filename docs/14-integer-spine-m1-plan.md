# Integer Spine — Milestone 1: Pipeline Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Compile a straight-line 8-bit program (`volatile unsigned char in; volatile unsigned char out; void main(void) { out = in + 1; }`) end-to-end through all ten pipeline stage crates to Intel HEX that runs correctly in `pic14-sim` (`out == in + 1`).

**Architecture:** Ten stage crates (`driver`, `irparse`, `wholeprog`, `legalize`, `callgraph`, `alloc`, `isel`, `banking`, `peephole`, `asm`), each a text boundary (reads text in, writes text out), plus a shared `crates/ir` crate defining the IR types and the canonical IR text format. The `pic14-sim` crate (already on master) is the oracle. `asm` is our own assembler (ADR-002) — `gpasm` is a test-time cross-check only.

**Tech Stack:** Rust 1.97.1 (workspace), clang 20.1.8 (pinned, external front end), `pic14-sim`, `gpasm` 1.5.2 (test oracle).

**Spec:** [`docs/12-backend-design.md`](../12-backend-design.md) §1 (pipeline) and §4 phase 2.

## Global Constraints

- Build/test with `nix develop --command cargo …`; never `apt install` toolchain deps.
- clang is driven via `$PIC8_CLANG_UNWRAPPED` with `-resource-dir "$PIC8_CLANG_RESOURCE_DIR"` (flake env vars; see docs/09).
- Conventional commits, single line, ≤ 3 lines.
- No external assembler/linker in the product; `gpasm` is external-process test-only. GPL tools never linked.
- Every stage boundary is a text artifact: each stage binary reads a file path (or stdin) and writes text to a file path (or stdout). No stage imports another stage's code — they communicate only via the text formats (the shared `crates/ir` crate defines the IR text format; everything else is plain text).
- New files must be `git add`ed before `nix develop` sees them.
- Unsupported constructs fail loudly with a clear `panic!` message, never silently miscompile.

## The IR text format (defined by `crates/ir`, used by every stage)

Line-based, one instruction per line, normalized (no LLVM attributes, metadata, or source info):

```
global <name> <ty>                    ; ty ∈ {i1, i8, i16}; e.g. "global in i8"
fn <name>(<ty> %<p>, ...) -> <ty>|<void>   ; e.g. "fn main() -> void"
block <label>:                        ; first block label is the entry
<inst>                                ; one per line
```

Instructions (v1 subset — anything else panics):

```
%d = load <ty> <ptr>                  ; ptr = @name | %name
store <ty> <val> <ptr>
%d = <binop> <ty> <a> <b>             ; binop ∈ add,sub,and,or,xor
ret <ty> <val> | ret void
br <label> | br i1 <cond> <t> <f>     ; (later milestones)
%d = zext <ty> %v to <ty>             ; (later milestones)
%d = trunc <ty> %v to <ty>            ; (later milestones)
%d = icmp <pred> <ty> <a> <b>         ; (later milestones)
%d = select i1 <c> <ty> <a> <b>       ; (later milestones)
%d = call <ty> @<fn>(<ty> <val>, ...) ; (later milestones)
%d = gep @<base> <val>                ; (later milestones)
```

`<val>` is `%name`, `@name`, or an integer literal. `%d`/`%name` are SSA names. Globals carry an address annotation after allocation: `global in i8 @0x20`. Defining instructions carry an address annotation after allocation: `%1 = load i8 @in @0x70`.

---

### Task 1: `crates/ir` — IR types and canonical text format

**Files:**
- Create: `crates/ir/Cargo.toml`
- Create: `crates/ir/src/lib.rs`
- Test: `crates/ir/tests/roundtrip.rs`
- Modify: `Cargo.toml` (add `"crates/ir"` to workspace members)

**Interfaces:**
- Produces: `pub enum Ty { I1, I8, I16 }` (with `bytes()`); `pub enum Val { Reg(String), Const(i64), Global(String) }`; `pub enum Inst { Load{dst,ty,ptr}, Store{ty,val,ptr}, Bin{dst,op,ty,a,b}, Ret{val} }` (plus the later-milestone variants as `unimplemented!()`-free placeholders only if trivial — otherwise omit for v1 and add in later plans); `pub struct Global { name, is_const, addr: Option<u8> }`; `pub struct Func { name, ret, params, blocks }`; `pub struct Block { label, insts }`; `pub struct Module { globals, funcs }`.
- `pub fn parse(text: &str) -> Module` and `pub fn serialize(m: &Module) -> String` — inverse, canonical.

- [ ] **Step 1: Write the failing test**

`crates/ir/tests/roundtrip.rs`:

```rust
use ir::{parse, serialize};

#[test]
fn roundtrips_a_straight_line_program() {
    let text = "global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n";
    let m = parse(text);
    let out = serialize(&m);
    let m2 = parse(&out);
    assert_eq!(serialize(&m2), out); // stable fixed point
    assert!(out.contains("%2 = add i8 %1, 1"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `nix develop --command cargo test -p ir`
Expected: FAIL, `ir` crate not found.

- [ ] **Step 3: Add the crate and implement**

`crates/ir/Cargo.toml`:

```toml
[package]
name = "ir"
version = "0.1.0"
edition = "2021"
publish = false
```

`crates/ir/src/lib.rs`:

```rust
//! Canonical IR text format shared by all pipeline stages. Text boundary:
//! every stage reads IR text in and writes IR text out.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ty { I1, I8, I16 }
impl Ty {
    pub fn bytes(self) -> u8 { match self { Ty::I1 | Ty::I8 => 1, Ty::I16 => 2 } }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Val { Reg(String), Const(i64), Global(String) }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp { Add, Sub, And, Or, Xor }

#[derive(Clone, Debug)]
pub struct Load { pub dst: String, pub ty: Ty, pub ptr: String } // ptr = "@name" or "%name"
#[derive(Clone, Debug)]
pub struct Store { pub ty: Ty, pub val: Val, pub ptr: String }
#[derive(Clone, Debug)]
pub struct Bin { pub dst: String, pub op: BinOp, pub ty: Ty, pub a: Val, pub b: Val }

#[derive(Clone, Debug)]
pub enum Inst {
    Load(Load),
    Store(Store),
    Bin(Bin),
    Ret(Option<(Ty, Val)>),
}

#[derive(Clone, Debug)]
pub struct Block { pub label: String, pub insts: Vec<Inst> }

#[derive(Clone, Debug)]
pub struct Func { pub name: String, pub ret: Option<Ty>, pub params: Vec<(Ty, String)>, pub blocks: Vec<Block> }

#[derive(Clone, Debug)]
pub struct Global { pub name: String, pub is_const: bool, pub addr: Option<u8> }

#[derive(Clone, Debug)]
pub struct Module { pub globals: Vec<Global>, pub funcs: Vec<Func> }

fn val_str(v: &Val) -> String {
    match v { Val::Reg(r) => format!("%{r}"), Val::Const(k) => k.to_string(), Val::Global(g) => format!("@{g}") }
}

pub fn serialize(m: &Module) -> String {
    let mut out = String::new();
    for g in &m.globals {
        let kind = if g.is_const { "const" } else { "global" };
        match g.addr { Some(a) => out.push_str(&format!("{kind} {} @0x{a:02X}\n", g.name)), None => out.push_str(&format!("{kind} {}\n", g.name)) }
    }
    for f in &m.funcs {
        let params: Vec<String> = f.params.iter().map(|(t, n)| format!("{t:?} %{n}").replace("I8", "i8").replace("I16", "i16").replace("I1", "i1")).collect();
        let ret = match f.ret { Some(t) => ty_str(t), None => "void".to_string() };
        out.push_str(&format!("fn {}({}) -> {ret}\n", f.name, params.join(", ")));
        for b in &f.blocks {
            out.push_str(&format!("  block {}:\n", b.label));
            for i in &b.insts {
                out.push_str(&format!("    {}\n", inst_str(i)));
            }
        }
    }
    out
}

fn ty_str(t: Ty) -> String { match t { Ty::I1 => "i1".into(), Ty::I8 => "i8".into(), Ty::I16 => "i16".into() } }

fn inst_str(i: &Inst) -> String {
    match i {
        Inst::Load(l) => format!("{} = load {} {}", l.dst, ty_str(l.ty), l.ptr),
        Inst::Store(s) => format!("store {} {} {}", ty_str(s.ty), val_str(&s.val), s.ptr),
        Inst::Bin(b) => format!("{} = {} {} {} {}", b.dst, op_str(b.op), ty_str(b.ty), val_str(&b.a), val_str(&b.b)),
        Inst::Ret(None) => "ret void".into(),
        Inst::Ret(Some((t, v))) => format!("ret {} {}", ty_str(*t), val_str(v)),
    }
}

fn op_str(o: BinOp) -> &'static str { match o { BinOp::Add => "add", BinOp::Sub => "sub", BinOp::And => "and", BinOp::Or => "or", BinOp::Xor => "xor" } }

pub fn parse(text: &str) -> Module {
    let mut globals = Vec::new();
    let mut funcs = Vec::new();
    let mut cur_func: Option<Func> = None;
    let mut cur_block: Option<Block> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if let Some(rest) = line.strip_prefix("global ").or_else(|| line.strip_prefix("const ")) {
            let is_const = line.starts_with("const ");
            let parts: Vec<&str> = rest.split_whitespace().collect();
            let (name, addr) = if parts.len() >= 2 { (parts[0], parse_addr(parts[1])) } else { (parts[0], None) };
            globals.push(Global { name: name.to_string(), is_const, addr });
        } else if line.starts_with("fn ") {
            let rest = &line[3..];
            let open = rest.find('(').unwrap();
            let name = rest[..open].trim().to_string();
            let close = rest.rfind(')').unwrap();
            let sig = &rest[open + 1..close];
            let ret = rest[close + 1..].trim().trim_start_matches("->").trim();
            let params = if sig.trim().is_empty() { vec![] } else {
                sig.split(',').map(|p| { let mut it = p.trim().split_whitespace(); let t = parse_ty(it.next().unwrap()); let n = it.next().unwrap().trim_start_matches('%').to_string(); (t, n) }).collect()
            };
            if let Some(f) = cur_func.take() { funcs.push(f); }
            cur_func = Some(Func { name, ret: if ret == "void" { None } else { Some(parse_ty(ret)) }, params, blocks: Vec::new() });
            cur_block = None;
        } else if line.starts_with("block ") {
            let label = line["block ".len()..].trim_end_matches(':').to_string();
            cur_block = Some(Block { label, insts: Vec::new() });
        } else {
            let inst = parse_inst(line);
            match (&mut cur_func, &mut cur_block) {
                (Some(f), Some(b)) => b.insts.push(inst),
                (Some(f), None) => panic!("instruction before any block: {line}"),
                (None, _) => panic!("instruction outside a function: {line}"),
            }
        }
    }
    if let Some(f) = cur_func.take() { funcs.push(f); }
    Module { globals, funcs }
}

fn parse_ty(s: &str) -> Ty { match s { "i1" => Ty::I1, "i8" => Ty::I8, "i16" => Ty::I16, other => panic!("unsupported type {other}") } }
fn parse_addr(s: &str) -> Option<u8> { s.strip_prefix('@').map(|h| u8::from_str_radix(h.trim_start_matches("0x"), 16).unwrap()) }
fn parse_val(s: &str) -> Val {
    if let Some(r) = s.strip_prefix('%') { Val::Reg(r.to_string()) }
    else if let Some(g) = s.strip_prefix('@') { Val::Global(g.to_string()) }
    else { Val::Const(s.parse().unwrap_or_else(|_| panic!("bad value {s}"))) }
}
fn parse_inst(line: &str) -> Inst {
    if let Some(rest) = line.strip_prefix("store ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        return Inst::Store(Store { ty: parse_ty(parts[0]), val: parse_val(parts[1]), ptr: parts[2].to_string() });
    }
    if let Some(rest) = line.strip_prefix("ret ") {
        if rest == "void" { return Inst::Ret(None); }
        let mut it = rest.split_whitespace();
        let t = parse_ty(it.next().unwrap());
        return Inst::Ret(Some((t, parse_val(it.next().unwrap()))));
    }
    // defining instruction: %d = op ...
    let eq = line.find(" = ").unwrap();
    let dst = line[..eq].trim_start_matches('%').to_string();
    let body = line[eq + 3..].trim();
    if let Some(rest) = body.strip_prefix("load ") {
        let mut it = rest.split_whitespace();
        let t = parse_ty(it.next().unwrap());
        let ptr = it.next().unwrap().to_string();
        return Inst::Load(Load { dst, ty: t, ptr });
    }
    let mut it = body.split_whitespace();
    let op = it.next().unwrap();
    let t = parse_ty(it.next().unwrap());
    let a = parse_val(it.next().unwrap());
    let b = parse_val(it.next().unwrap());
    let op = match op { "add" => BinOp::Add, "sub" => BinOp::Sub, "and" => BinOp::And, "or" => BinOp::Or, "xor" => BinOp::Xor, other => panic!("unsupported op {other}") };
    Inst::Bin(Bin { dst, op, ty: t, a, b })
}
```

Also update root `Cargo.toml` workspace members to include `"crates/ir"`.

- [ ] **Step 4: Run test to verify it passes**

Run: `nix develop --command cargo test -p ir`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/ir
git commit -m "feat(ir): define canonical IR text format with round-trip"
```

---

### Task 2: `irparse` — `.ll` to IR text

**Files:**
- Create: `crates/irparse/Cargo.toml`, `crates/irparse/src/lib.rs`, `crates/irparse/src/bin/irparse.rs`
- Test: `crates/irparse/tests/parse_ll.rs`
- Modify: `Cargo.toml` (add member)

**Interfaces:**
- Consumes: `ir` crate (`Module`, `parse`, `serialize`).
- Produces: `pub fn parse_ll(src: &str) -> ir::Module`; binary `irparse <in.ll> <out.ir>`.

- [ ] **Step 1: Write the failing test**

`crates/irparse/tests/parse_ll.rs`:

```rust
use irparse::parse_ll;

const LL: &str = r#"
@in = dso_local global i8 0, align 1
@out = dso_local global i8 0, align 1
define dso_local void @main() {
  %1 = load volatile i8, ptr @in, align 1
  %2 = add nsw i8 %1, 1
  store volatile i8 %2, ptr @out, align 1
  ret void
}
"#;

#[test]
fn parses_straight_line_ll() {
    let m = parse_ll(LL);
    assert_eq!(m.globals.len(), 2);
    assert_eq!(m.funcs.len(), 1);
    assert_eq!(m.funcs[0].blocks.len(), 1);
    assert_eq!(m.funcs[0].blocks[0].insts.len(), 4);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `nix develop --command cargo test -p irparse`
Expected: FAIL, crate not found.

- [ ] **Step 3: Implement (port the spike parser, attribute stripping required)**

`crates/irparse/src/lib.rs` — port `parse`/`parse_inst`/`strip_attrs`/`parse_val` from the verified `spike/src/ir.rs` (on disk), adapting the `Inst` constructors to the `ir` crate's struct form. The parser must strip unmodeled attributes (`noundef`, `nsw`, `nuw`, `nneg`, `volatile`, `inbounds`, `align(...)`, `dso_local`, `local_unnamed_addr`, metadata `, !tbaa !2`) and panic loudly on anything else. Handle: `load`/`store` (global and SSA pointer operands), `add`/`sub`/`and`/`or`/`xor`, `ret`. For milestone 1, `getelementptr`, `phi`, `select`, `call`, `icmp`, `zext`, `trunc`, `br` may panic with `"SPIKE LIMIT: unsupported for milestone 1"` (later milestones implement them).

`crates/irparse/src/bin/irparse.rs`:

```rust
use std::fs;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = fs::read_to_string(&args[1]).expect("read input");
    let m = irparse::parse_ll(&src);
    fs::write(&args[2], ir::serialize(&m)).expect("write output");
}
```

`crates/irparse/Cargo.toml` needs `[[bin]] name = "irparse" path = "src/bin/irparse.rs"` and deps `ir`.

- [ ] **Step 4: Run test to verify it passes**

Run: `nix develop --command cargo test -p irparse`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/irparse
git commit -m "feat(irparse): parse LLVM IR text into canonical IR"
```

---

### Task 3: `wholeprog` — single-module validation pass-through

**Files:**
- Create: `crates/wholeprog/Cargo.toml`, `crates/wholeprog/src/lib.rs`, `crates/wholeprog/src/bin/wholeprog.rs`
- Test: `crates/wholeprog/tests/merge.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: `ir` (`Module`, `parse`, `serialize`).
- Produces: `pub fn merge(m: Module) -> Module` — v1: validates it is a single module (panic if `m.funcs.is_empty()`, duplicate global/function names) and returns it unchanged.

- [ ] **Step 1: Write the failing test**

`crates/wholeprog/tests/merge.rs`:

```rust
use wholeprog::merge;
use ir::parse;

#[test]
fn passes_single_module_through() {
    let m = parse("global in i8\nfn main() -> void\n  block entry:\n    ret void\n");
    let out = merge(m);
    assert_eq!(out.funcs.len(), 1);
}

#[test]
#[should_panic]
fn rejects_empty_module() {
    merge(parse(""));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop --command cargo test -p wholeprog`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
use ir::{Module, parse, serialize};
pub fn merge(m: Module) -> Module {
    assert!(!m.funcs.is_empty(), "wholeprog: no functions in module");
    m
}
```
Binary `wholeprog <in.ir> <out.ir>` reads text, calls `merge(parse(...))`, writes `serialize(...)`.

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop --command cargo test -p wholeprog`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/wholeprog
git commit -m "feat(wholeprog): single-module merge validation"
```

---

### Task 4: `legalize` — type-width validation pass-through

**Files:**
- Create: `crates/legalize/Cargo.toml`, `crates/legalize/src/lib.rs`, `crates/legalize/src/bin/legalize.rs`
- Test: `crates/legalize/tests/legalize.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: `ir`.
- Produces: `pub fn legalize(m: Module) -> Module` — v1: validates every type is `i1`/`i8`/`i16` (panic on `i17` etc. with a clear message) and returns `m`. Later milestones add widening/narrowing.

- [ ] **Step 1: Write the failing test**

`crates/legalize/tests/legalize.rs`:

```rust
use legalize::legalize;
use ir::parse;

#[test]
fn passes_8_bit_through() {
    let m = parse("global in i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    ret void\n");
    assert_eq!(legalize(m).funcs.len(), 1);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop --command cargo test -p legalize`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
use ir::{Module, parse, serialize};
pub fn legalize(m: Module) -> Module {
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                // The `ir` parser already rejects non-i1/i8/i16 types, so v1 is a
                // pass-through boundary that later milestones extend (i16->i8 lowering,
                // runtime calls for mul/div).
            }
        }
    }
    m
}
```
Binary `legalize <in.ir> <out.ir>`.

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop --command cargo test -p legalize`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/legalize
git commit -m "feat(legalize): type-width validation boundary"
```

---

### Task 5: `callgraph` — call graph, recursion and depth checks

**Files:**
- Create: `crates/callgraph/Cargo.toml`, `crates/callgraph/src/lib.rs`, `crates/callgraph/src/bin/callgraph.rs`
- Test: `crates/callgraph/tests/graph.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: `ir` (`Module`).
- Produces: `pub struct CallGraph { pub edges: Vec<(String, String)>, pub max_depth: usize }`; `pub fn build(m: &Module) -> CallGraph`; `pub fn check_depth(g: &CallGraph, limit: usize)` (panics if `max_depth > limit`); binary writes the call graph as text (`fn -> callee` per line) and a `depth N` line, and panics on recursion (v1: any cycle) or depth > 8.

- [ ] **Step 1: Write the failing test**

`crates/callgraph/tests/graph.rs`:

```rust
use callgraph::build;
use ir::parse;

#[test]
fn single_function_has_no_edges() {
    let m = parse("fn main() -> void\n  block entry:\n    ret void\n");
    let g = build(&m);
    assert!(g.edges.is_empty());
    assert_eq!(g.max_depth, 1);
}

```
(The recursion-rejection and depth-limit tests belong to the call-graph-with-calls milestone, which adds the `call` IR instruction; milestone 1 has no call edges.)

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop --command cargo test -p callgraph`
Expected: FAIL.

- [ ] **Step 3: Implement (no calls in milestone 1)**

```rust
use ir::Module;
pub struct CallGraph { pub edges: Vec<(String, String)>, pub max_depth: usize }
pub fn build(_m: &Module) -> CallGraph {
    // Milestone 1: no call instructions exist in the straight-line subset, so the
    // graph is a forest of depth 1. The call milestone adds edges from call sites.
    CallGraph { edges: Vec::new(), max_depth: 1 }
}
pub fn check_depth(g: &CallGraph, limit: usize) {
    assert!(g.max_depth <= limit, "callgraph: depth {} exceeds hardware stack {limit}", g.max_depth);
}
```
Binary `callgraph <in.ir> <out.cg>` writes `depth 1` (and `fn main` if present).

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop --command cargo test -p callgraph`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/callgraph
git commit -m "feat(callgraph): call graph and stack-depth check boundary"
```

---

### Task 6: `alloc` — addresses for globals and locals

**Files:**
- Create: `crates/alloc/Cargo.toml`, `crates/alloc/src/lib.rs`, `crates/alloc/src/bin/alloc.rs`
- Test: `crates/alloc/tests/alloc.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: `ir` (`Module`); the bank-0 layout from the design: RAM globals start at `0x20`, scratch/retval immediately after, common RAM `0x70..=0x7F` for locals (imaginary registers), then bank-0 GPR `0x25..` overflow.
- Produces: `pub fn allocate(mut m: Module) -> Module` — assigns `Global.addr` for every non-const global (sequential from `0x20`), and appends `@0xNN` address annotations to defining instructions' destination names in the serialized text for every local (assign from `0x70` upward, overflow to bank-0 GPR after `0x7F`). `const` globals get no address (flash; later milestone).

**Address-annotation mechanism:** `allocate` returns the module with `Global.addr` set; the binary writes IR text via `serialize` — so the plan extends `ir::serialize` (Task 1) to emit `@0xNN` on globals that have `addr`, and this task extends `ir::serialize`/the IR text to carry per-instruction addresses. Simpler v1: **`alloc` writes the address map as a separate text file** `<out>.map` (one `name 0xNN` per line) alongside the IR text, and `isel` (Task 7) reads the map. Decision: separate `.map` file (keeps the IR text format unchanged).

- [ ] **Step 1: Write the failing test**

`crates/alloc/tests/alloc.rs`:

```rust
use alloc::allocate;
use ir::parse;

#[test]
fn globals_get_bank0_addresses() {
    let m = parse("global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    ret void\n");
    let out = allocate(m);
    assert_eq!(out.globals[0].addr, Some(0x20));
    assert_eq!(out.globals[1].addr, Some(0x21));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop --command cargo test -p alloc`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
use ir::{Module, Global};
pub const GLOBAL_START: u8 = 0x20;

pub fn allocate(mut m: Module) -> Module {
    let mut addr = GLOBAL_START;
    for g in &mut m.globals {
        if !g.is_const {
            g.addr = Some(addr);
            addr += 1; // i8 globals; i16 -> +2 (later milestone)
        }
    }
    m
}

pub fn address_map(m: &Module) -> String {
    let mut out = String::new();
    for g in &m.globals {
        if let Some(a) = g.addr {
            out.push_str(&format!("global {} 0x{a:02X}\n", g.name));
        }
    }
    out
}
```
Binary `alloc <in.ir> <out.ir> <out.map>`: writes `serialize(&allocate(parse(...)))` and `address_map(&m)`. (Local addresses land with the isel milestone, which assigns SSA-value slots directly for straight-line code; the map carries globals for now.)

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop --command cargo test -p alloc`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/alloc
git commit -m "feat(alloc): assign bank-0 addresses to globals"
```

---

### Task 7: `isel` — straight-line 8-bit codegen to `.asm`

**Files:**
- Create: `crates/isel/Cargo.toml`, `crates/isel/src/lib.rs`, `crates/isel/src/bin/isel.rs`
- Test: `crates/isel/tests/isel.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: `ir` (`Module`, `parse`), the address map file (global names → addresses).
- Produces: `pub fn select(m: &ir::Module, addrs: &std::collections::HashMap<String, u8>) -> String` — PIC14 assembly text (the `.asm` format from the spike: `    MOVF 0x20, W` etc., plus `STATUS equ 0x03`, `org 0x0000`, `goto __start`, functions as labels, `__start: CALL main / SLEEP / end`).

**Instruction selection (8-bit straight-line):**
- `%d = load <ty> @g` → `MOVF <g>, W` / `MOVWF <d>` (d assigned a slot: locals from 0x70 up, per `alloc`).
- `store <ty> <val> @g` → `MOVF <src>, W` / `MOVWF <g>`.
- `%d = add i8 <a> <b>` → byte add: `MOVF <b>, W` / `ADDWF <a>, W` / `MOVWF <d>`.
- `ret void` → `RETURN`.
- Slots: assign each SSA destination an address in a per-function map (common RAM 0x70→0x7F, then bank-0 GPR from 0x25); params/globals via the address map.

- [ ] **Step 1: Write the failing test**

`crates/isel/tests/isel.rs`:

```rust
use isel::select;
use ir::parse;
use std::collections::HashMap;

#[test]
fn emits_add_for_in_plus_one() {
    let m = parse("global in i8\nglobal out i8\nfn main() -> void\n  block entry:\n    %1 = load i8 @in\n    %2 = add i8 %1, 1\n    store i8 %2 @out\n    ret void\n");
    let mut addrs = HashMap::new();
    addrs.insert("in".to_string(), 0x20u8);
    addrs.insert("out".to_string(), 0x21u8);
    let asm = select(&m, &addrs);
    assert!(asm.contains("MOVF 0x20, W"));
    assert!(asm.contains("ADDLW 0x01"));
    assert!(asm.contains("MOVWF 0x21"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop --command cargo test -p isel`
Expected: FAIL.

- [ ] **Step 3: Implement** (port the verified straight-line codegen from `spike/src/codegen.rs` — the `emit_load_byte`/`MOVF`/`MOVWF`/`ADDLW` patterns; keep only 8-bit load/store/add/sub/and/or/xor + ret for milestone 1; panic loudly on anything else)

`crates/isel/src/lib.rs` (key parts):

```rust
use ir::{Module, Inst, Val};
use std::collections::HashMap;

const COMMON_START: u8 = 0x70;
const BANK0_START: u8 = 0x25;

pub fn select(m: &Module, addrs: &HashMap<String, u8>) -> String {
    let mut out = vec![
        "; pic8 -- integer spine milestone 1 (throwaway isel)".to_string(),
        "    list p=16f877a".to_string(),
        "    radix hex".to_string(),
        "STATUS equ 0x03".to_string(),
        "".to_string(),
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ];
    for f in &m.funcs {
        out.push(format!("{0}:", f.name));
        let mut slots: HashMap<String, u8> = HashMap::new();
        let mut next = COMMON_START;
        for b in &f.blocks {
            out.push(format!("{0}_L{1}:", f.name, b.label));
            for i in &b.insts {
                match i {
                    Inst::Load(l) => {
                        let (src, dst) = (ptr_addr(&l.ptr, addrs, &slots), slot(&mut slots, &mut next, &l.dst));
                        out.push(format!("    MOVF 0x{src:02X}, W"));
                        out.push(format!("    MOVWF 0x{dst:02X}"));
                    }
                    Inst::Store(s) => {
                        let (dst, src) = (ptr_addr(&s.ptr, addrs, &slots), val_addr(&s.val, addrs, &slots));
                        out.push(format!("    MOVF 0x{src:02X}, W"));
                        out.push(format!("    MOVWF 0x{dst:02X}"));
                    }
                    Inst::Bin(b) => {
                        let (da, aa, ba) = (slot(&mut slots, &mut next, &b.dst), val_addr(&b.a, addrs, &slots), val_addr(&b.b, addrs, &slots));
                        match (b.op, &b.b) {
                            (ir::BinOp::Add, Val::Const(k)) => {
                                out.push(format!("    MOVF 0x{aa:02X}, W"));
                                out.push(format!("    ADDLW 0x{:02X}", (*k as u8)));
                                out.push(format!("    MOVWF 0x{da:02X}"));
                            }
                            (ir::BinOp::Add, _) => {
                                out.push(format!("    MOVF 0x{ba:02X}, W"));
                                out.push(format!("    ADDWF 0x{aa:02X}, W"));
                                out.push(format!("    MOVWF 0x{da:02X}"));
                            }
                            _ => panic!("isel: unsupported binop for milestone 1"),
                        }
                    }
                    Inst::Ret(None) => out.push("    RETURN".to_string()),
                    _ => panic!("isel: unsupported instruction for milestone 1"),
                }
            }
        }
    }
    out.push("__start:".to_string());
    out.push("    CALL main".to_string());
    out.push("    SLEEP".to_string());
    out.push("".to_string());
    out.push("    end".to_string());
    out.join("\n")
}

fn slot(slots: &mut HashMap<String, u8>, next: &mut u8, name: &str) -> u8 {
    if let Some(&a) = slots.get(name) { return a; }
    let a = *next;
    *next += 1;
    if *next > 0x80 { *next = BANK0_START; }
    slots.insert(name.to_string(), a);
    a
}
fn val_addr(v: &Val, addrs: &HashMap<String, u8>, slots: &HashMap<String, u8>) -> u8 {
    match v { Val::Reg(r) => *slots.get(r).unwrap_or_else(|| panic!("isel: no slot for %{r}")), Val::Global(g) => *addrs.get(g).unwrap_or_else(|| panic!("isel: no address for @{g}")), Val::Const(k) => { assert!(*k >= 0 && *k <= 255); *k as u8 } }
}
fn ptr_addr(p: &str, addrs: &HashMap<String, u8>, slots: &HashMap<String, u8>) -> u8 {
    if let Some(g) = p.strip_prefix('@') { *addrs.get(g).unwrap() } else { *slots.get(p.trim_start_matches('%')).unwrap() }
}
```

(Note: `slot`'s overflow handling is a v1 approximation — the common-RAM pressure and spill design lands in the overlay-allocation milestone. `Val::Const` is only valid on the RHS `b` operand here; the add-const arm handles it; the general `val_addr` const path is for completeness.)

Binary `isel <in.ir> <in.map> <out.asm>` reads IR text + address map, writes `.asm`.

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop --command cargo test -p isel`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/isel
git commit -m "feat(isel): straight-line 8-bit instruction selection"
```

---

### Task 8: `banking` — bank-0-only validation

**Files:**
- Create: `crates/banking/Cargo.toml`, `crates/banking/src/lib.rs`, `crates/banking/src/bin/banking.rs`
- Test: `crates/banking/tests/banking.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: `.asm` text.
- Produces: `pub fn assign_banks(asm: &str) -> String` — v1: asserts every file-register operand is `0x00..=0x7F` (bank 0 / common; no `BANKSEL` needed) and returns the text unchanged. Panics on any operand `>= 0x80`.

- [ ] **Step 1: Write the failing test**

`crates/banking/tests/banking.rs`:

```rust
use banking::assign_banks;

#[test]
fn passes_bank0_asm_through() {
    let asm = "    MOVF 0x20, W\n    MOVWF 0x21\n";
    assert_eq!(assign_banks(asm), asm);
}

#[test]
#[should_panic]
fn rejects_bank_operand() {
    assign_banks("    MOVF 0x80, W\n");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop --command cargo test -p banking`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
pub fn assign_banks(asm: &str) -> String {
    for line in asm.lines() {
        for tok in line.split_whitespace() {
            if let Some(hex) = tok.strip_prefix("0x") {
                let v = u16::from_str_radix(hex, 16).unwrap();
                if v >= 0x80 { panic!("banking: operand 0x{v:02X} is outside bank 0/1/2/3 GPR range (milestone 1: bank 0 only)"); }
            }
        }
    }
    asm.to_string()
}
```
Binary `banking <in.asm> <out.asm>`.

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop --command cargo test -p banking`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/banking
git commit -m "feat(banking): bank-0 validation boundary"
```

---

### Task 9: `peephole` — pass-through boundary

**Files:**
- Create: `crates/peephole/Cargo.toml`, `crates/peephole/src/lib.rs`, `crates/peephole/src/bin/peephole.rs`
- Test: `crates/peephole/tests/peephole.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Produces: `pub fn optimize(asm: &str) -> String` — v1 pass-through (returns input). Later milestones add pattern cleanup.

- [ ] **Step 1: Write the failing test**

`crates/peephole/tests/peephole.rs`:

```rust
use peephole::optimize;
#[test]
fn passes_through() {
    let asm = "    NOP\n";
    assert_eq!(optimize(asm), asm);
}
```

- [ ] **Step 2-4: Implement, verify, pass** (mirror Tasks 3/8: crate + lib `pub fn optimize(asm: &str) -> String { asm.to_string() }` + binary + test). Commit `feat(peephole): pass-through boundary`.

---

### Task 10: `asm` — our own assembler to Intel HEX

**Files:**
- Create: `crates/asm/Cargo.toml`, `crates/asm/src/lib.rs`, `crates/asm/src/bin/asm.rs`
- Test: `crates/asm/tests/assemble.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Produces: `pub fn assemble(src: &str) -> Vec<u16>` (14-bit words, indexed by address) and `pub fn to_hex(words: &[u16]) -> String` (Intel HEX, the format `pic14_sim::parse_hex` already decodes: two little-endian bytes per word at `addr*2`, `04` extended-linear-address header, `01` EOF).

**Encodings (from the 16F877A datasheet; the simulator already validates these):**
- `NOP` 0x0000 · `RETURN` 0x0008 · `SLEEP` 0x0063 · `CLRWDT` 0x0064
- `MOVWF f` 0x0080|f · `MOVF f,W` 0x0800|f · `CLRF f` 0x0180|f
- `ADDWF f,W` 0x0700|f · `SUBWF f,W` 0x0200|f · `ANDWF f,W` 0x0500|f · `IORWF f,W` 0x0400|f · `XORWF f,W` 0x0600|f
- `MOVLW k` 0x3000|k · `ADDLW k` 0x3E00|k · `ANDLW k` 0x3900|k · `IORLW k` 0x3800|k · `XORLW k` 0x3A00|k · `SUBLW k` 0x3C00|k · `RETLW k` 0x3400|k
- `BTFSC f,b` 0x1800|(b<<7)|f · `BTFSS f,b` 0x1C00|(b<<7)|f · `BCF f,b` 0x1000|(b<<7)|f · `BSF f,b` 0x1400|(b<<7)|f
- `GOTO k` 0x2800|k · `CALL k` 0x2000|k

Assembler needs a symbol table (labels → word addresses, `LOW(label)` → low byte, `HIGH(label)` → high byte), two passes (first pass assigns addresses; second pass resolves). `.asm` directives: `list`, `radix`, `equ`, `org`, `end`, labels ending `:`, and `;` comments.

- [ ] **Step 1: Write the failing test**

`crates/asm/tests/assemble.rs`:

```rust
use asm::assemble;

#[test]
fn assembles_movf_add_movwf() {
    let src = "    org 0x0000\n    goto __start\n__start:\n    movf 0x20, W\n    movlw 0x01\n    addwf 0x20, W\n    movwf 0x21\n    sleep\n    end\n";
    let words = assemble(src);
    assert_eq!(words[0], 0x2801); // goto __start (word 1)
    assert_eq!(words[1], 0x0820); // movf 0x20, W
    assert_eq!(words[2], 0x3001); // movlw 0x01
    assert_eq!(words[3], 0x0720); // addwf 0x20, W
    assert_eq!(words[4], 0x00A1); // movwf 0x21
    assert_eq!(words[5], 0x0063); // sleep
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop --command cargo test -p asm`
Expected: FAIL.

- [ ] **Step 3: Implement**

`crates/asm/src/lib.rs` — a two-pass line assembler:

```rust
pub fn assemble(src: &str) -> Vec<u16> {
    let mut words: Vec<u16> = Vec::new();
    let mut symbols: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut org = 0usize;
    // Pass 1: labels, org, equ; measure size.
    let mut lines: Vec<(usize, String)> = Vec::new(); // (address, mnemonic line)
    for raw in src.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() { continue; }
        if line.starts_with("list") || line.starts_with("radix") { continue; }
        if let Some(rest) = line.strip_prefix("org ") {
            org = parse_num(rest.trim());
            continue;
        }
        if let Some(rest) = line.strip_prefix("end") {
            break;
        }
        if let Some(label) = line.strip_suffix(':') {
            symbols.insert(label.trim().to_string(), org);
            continue;
        }
        if let Some(eq) = line.find(" equ ") {
            let (name, val) = line.split_at(eq);
            symbols.insert(name.trim().to_string(), parse_num(val[" equ ".len()..].trim()));
            continue;
        }
        lines.push((org, line.to_string()));
        org += 1;
    }
    // Pass 2: encode.
    let mut out = vec![0u16; org];
    for (addr, line) in &lines {
        out[*addr] = encode(line, &symbols);
    }
    out
}

fn parse_num(s: &str) -> usize {
    if let Some(h) = s.strip_prefix("0x") { usize::from_str_radix(h, 16).unwrap() }
    else { s.parse().unwrap() }
}

fn encode(line: &str, sym: &std::collections::HashMap<String, usize>) -> u16 {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let mne = parts[0].to_ascii_uppercase();
    let op = parts.get(1).copied().unwrap_or("");
    let f = |s: &str| -> u16 { parse_num(s) as u16 & 0x7F };
    match mne.as_str() {
        "NOP" => 0x0000,
        "RETURN" => 0x0008,
        "SLEEP" => 0x0063,
        "CLRWDT" => 0x0064,
        "MOVWF" => 0x0080 | f(op),
        "CLRF" => 0x0180 | f(op),
        "MOVF" => 0x0800 | f(op),
        "ADDWF" => 0x0700 | f(op),
        "SUBWF" => 0x0200 | f(op),
        "ANDWF" => 0x0500 | f(op),
        "IORWF" => 0x0400 | f(op),
        "XORWF" => 0x0600 | f(op),
        "MOVLW" => 0x3000 | parse_num(op) as u16,
        "ADDLW" => 0x3E00 | parse_num(op) as u16,
        "ANDLW" => 0x3900 | parse_num(op) as u16,
        "IORLW" => 0x3800 | parse_num(op) as u16,
        "XORLW" => 0x3A00 | parse_num(op) as u16,
        "SUBLW" => 0x3C00 | parse_num(op) as u16,
        "RETLW" => 0x3400 | parse_num(op) as u16,
        "BTFSC" | "BTFSS" | "BCF" | "BSF" => {
            let (f, b) = op.split_once(',').unwrap();
            let base = match mne.as_str() { "BTFSC" => 0x1800, "BTFSS" => 0x1C00, "BCF" => 0x1000, _ => 0x1400 };
            base | ((parse_num(b.trim()) as u16 & 7) << 7) | f(f.trim())
        }
        "GOTO" => 0x2800 | (sym.get(op).copied().unwrap_or_else(|| parse_num(op)) as u16 & 0x7FF),
        "CALL" => 0x2000 | (sym.get(op).copied().unwrap_or_else(|| parse_num(op)) as u16 & 0x7FF),
        other => panic!("asm: unsupported mnemonic {other}"),
    }
}

/// Intel HEX from 14-bit words: little-endian pairs at word*2.
pub fn to_hex(words: &[u16]) -> String {
    let mut hex = String::new();
    // trim trailing zeros to the highest set word
    let hi = words.iter().rposition(|&w| w != 0).map(|i| i + 1).unwrap_or(0);
    let mut addr = 0usize;
    while addr < hi {
        let n = (hi - addr).min(16);
        let mut body = vec![0u8; 2 * n];
        for (i, w) in words[addr..addr + n].iter().enumerate() {
            body[2 * i] = (w & 0xFF) as u8;
            body[2 * i + 1] = ((w >> 8) & 0xFF) as u8;
        }
        let byte_addr = addr * 2;
        let mut rec = vec![n as u8, (byte_addr >> 8) as u8, (byte_addr & 0xFF) as u8, 0x00];
        rec.extend_from_slice(&body);
        let sum: u16 = rec.iter().map(|&b| b as u16).sum();
        rec.push((0x100 - (sum & 0xFF)) as u8);
        hex.push_str(":");
        for b in &rec { hex.push_str(&format!("{b:02X}")); }
        hex.push('\n');
        addr += n;
    }
    hex.push_str(":00000001FF\n");
    hex
}
```

Also provide `pub fn assemble_file_to_hex(src: &str) -> String { to_hex(&assemble(src)) }` and the binary `asm <in.asm> <out.hex>`. Note: `to_hex` must match `pic14_sim::parse_hex` byte order exactly (verified in the cross-check test below).

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop --command cargo test -p asm`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/asm
git commit -m "feat(asm): two-pass assembler to Intel HEX"
```

---

### Task 11: `driver` — end-to-end CLI

**Files:**
- Create: `crates/driver/Cargo.toml`, `crates/driver/src/main.rs`
- Modify: `Cargo.toml`
- Test: `crates/driver/tests/e2e.rs`

**Interfaces:**
- Consumes: all stage crates + `pic14_sim`.
- Produces: `driver <in.c> [out.hex]` — invokes clang (`$PIC8_CLANG_UNWRAPPED` with `-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc -resource-dir $PIC8_CLANG_RESOURCE_DIR`) → `.ll`; chains `irparse` → `wholeprog` → `legalize` → `callgraph` (depth check vs 8) → `alloc` → `isel` → `banking` → `peephole` → `asm` → `.hex`.

- [ ] **Step 1: Write the failing test**

`crates/driver/tests/e2e.rs`:

```rust
use std::process::Command;

#[test]
fn compiles_straight_line_program_end_to_end() {
    let out = Command::new(env!("CARGO_BIN_EXE_driver"))
        .args(["tests/fixtures/add.c", "tests/fixtures/add.hex"])
        .output().expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));
    // Simulate the output and assert out == in + 1
    let hex = std::fs::read_to_string("tests/fixtures/add.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[0x20] = 0x07; // in = 7
    p.run(1000);
    assert_eq!(p.ram()[0x21], 0x08); // out = 8
    assert!(p.halted());
}
```

Fixture `crates/driver/tests/fixtures/add.c`:

```c
volatile unsigned char in;
volatile unsigned char out;
void main(void) { out = in + 1; }
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop --command cargo test -p driver`
Expected: FAIL.

- [ ] **Step 3: Implement**

`crates/driver/src/main.rs` chains the stages. It may call the stage *libraries* directly (they are crate dependencies — the text-boundary constraint is about the *stages* not importing each other, not about the driver orchestrating them). The driver:

```rust
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let c_file = &args[1];
    let hex_out = args.get(2).map(String::as_str).unwrap_or("out.hex");
    // 1. clang: .c -> .ll
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let ll = Command::new(clang)
        .args(["-target", "msp430", "-O1", "-S", "-emit-llvm", "-ffreestanding", "-nostdinc", "-resource-dir", &resdir, "-o", "-", c_file])
        .output().expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();
    // 2-5. irparse -> wholeprog -> legalize -> callgraph
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, 8);
    // 6. alloc (address map for globals)
    m = alloc::allocate(m);
    let map = alloc::address_map(&m);
    // 7. isel
    let addrs: std::collections::HashMap<String, u8> = map.lines().map(|l| {
        let mut it = l.split_whitespace();
        let n = it.next().unwrap().to_string();
        let a = u8::from_str_radix(it.next().unwrap().trim_start_matches("0x"), 16).unwrap();
        (n, a)
    }).collect();
    let asm = isel::select(&m, &addrs);
    // 8-9. banking -> peephole
    let asm = banking::assign_banks(&asm);
    let asm = peephole::optimize(&asm);
    // 10. asm
    let hex = asm::assemble_file_to_hex(&asm);
    std::fs::write(hex_out, hex).expect("write hex");
}
```

`crates/driver/Cargo.toml` deps: `ir`, `irparse`, `wholeprog`, `legalize`, `callgraph`, `alloc`, `isel`, `banking`, `peephole`, `asm`. `dev-dependencies`: `pic14-sim`. Add `[dev-dependencies]` fixture path; the test uses `env!("CARGO_BIN_EXE_driver")` (Cargo provides it for integration tests).

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop --command cargo test -p driver`
Expected: PASS (end-to-end: `out == 8` for `in == 7`, halted).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/driver
git commit -m "feat(driver): chain the ten-stage pipeline end-to-end"
```

---

### Task 12: gpasm cross-check of our assembler

**Files:**
- Test: `crates/asm/tests/gpasm_cross.rs`
- Create: `crates/asm/tests/fixtures/add.asm`

**Interfaces:**
- Consumes: `asm::assemble`, `pic14_sim::parse_hex` + `Pic14`.
- Produces: an integration test proving our assembler agrees with `gpasm` on the same `.asm` (ADR-002's cross-check oracle).

- [ ] **Step 1: Write the fixture + test**

`crates/asm/tests/fixtures/add.asm` (identical to the `isel` output for `out = in + 1`):

```asm
    list p=16f877a
    radix hex
    org 0x0000
    goto __start
main:
    MOVF 0x20, W
    ADDLW 0x01
    MOVWF 0x21
    RETURN
__start:
    CALL main
    SLEEP
    end
```

`crates/asm/tests/gpasm_cross.rs`:

```rust
use asm::assemble;
use pic14_sim::{parse_hex, Pic14};
use std::process::Command;

fn gpasm() -> String { std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into()) }

#[test]
fn our_hex_matches_gpasm_and_runs() {
    let src = include_str!("fixtures/add.asm");
    let ours = asm::assemble_file_to_hex(src);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::write(format!("{dir}/add_ours.hex"), &ours).unwrap();
    let out = Command::new(gpasm())
        .args(["-p", "p16f877a", "add.asm", "-o", "add_gpasm.hex"])
        .current_dir(dir).output().expect("run gpasm");
    assert!(out.status.success());
    let theirs = std::fs::read_to_string(format!("{dir}/add_gpasm.hex")).unwrap();
    assert_eq!(ours.trim(), theirs.trim(), "our HEX differs from gpasm");
    // and it runs in the simulator
    let mut p = Pic14::new(parse_hex(&ours));
    p.ram_mut()[0x20] = 5;
    p.run(1000);
    assert_eq!(p.ram()[0x21], 6);
    assert!(p.halted());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop --command cargo test -p asm --test gpasm_cross`
Expected: FAIL if our HEX differs from gpasm's (debug the byte order / encodings until it matches).

- [ ] **Step 3: Fix until it passes** — if our encoding disagrees with gpasm, correct `crates/asm/src/lib.rs` encodings (the simulator's `pic14_sim::parse_hex` is the ground truth for byte order; the datasheet table in Task 10 for encodings).

- [ ] **Step 4: Verify full suite**

Run: `nix develop --command cargo test`
Expected: all crates' tests pass, including the driver e2e and this cross-check.

- [ ] **Step 5: Commit**

```bash
git add crates/asm/tests
git commit -m "test(asm): cross-check our assembler against gpasm"
```

---

## Self-review notes

- **Spec coverage:** milestone 1 covers all 10 stages of the approved pipeline (docs/12 §1) with text boundaries, the driver, our own assembler (ADR-002), and the phase-1 simulator as oracle. Control flow, calls, 16-bit arithmetic, overlay allocation, real banking, and the runtime library are deliberately later milestones of phase 2 (per docs/12 §4 phasing item 2); the `ir` crate's Inst enum is structured so those variants slot in without rework.
- **Deferred to later milestones (noted, not placeholders):** `getelementptr`, `phi`, `select`, `icmp`, `zext`, `trunc`, `call`, `br` in `irparse`/`isel` (panic loudly until implemented); overlay allocation (Task 7 uses naive sequential slots); real banking (Task 8 is bank-0-only); recursion/depth (Task 5 graph is empty for straight-line). Each later milestone extends the same text-boundary crates.
- **Type consistency:** `ir::Module`/`parse`/`serialize`, `irparse::parse_ll`, `wholeprog::merge`, `legalize::legalize`, `callgraph::{build, check_depth}`, `alloc::{allocate, address_map}`, `isel::select`, `banking::assign_banks`, `peephole::optimize`, `asm::{assemble, assemble_file_to_hex, to_hex}`, `pic14_sim::{parse_hex, Pic14, ram_mut}` — names stable across tasks.
- **The `ir` text format is the load-bearing interface.** Task 1 defines it; every later task consumes it. The spike (`spike/src/ir.rs`, `spike/src/codegen.rs`, on disk, gitignored) is the verified reference for the parsers/codegen ports.
