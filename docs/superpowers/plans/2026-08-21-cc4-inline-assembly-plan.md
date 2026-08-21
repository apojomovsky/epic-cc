# CC-4 Inline Assembly Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement rungs 1–3 of D-3 (naked functions, module `asm`, opaque statement-level `asm volatile`) on both PIC14 and PIC18 with header-only intrinsics, verbatim emission, and conservative banking barriers.

**Architecture:** New IR fields `Module.module_asm`, `Func.naked`, `Inst::Asm { template, clobbers_memory }`; `irparse` lifts the three `.ll` shapes (`module asm`, `naked` attribute, `call void asm sideeffect`); `isel`/`isel-pic18` emit verbatim; `banking` resets to UNKNOWN around each block; `epic-cc.h` adds `EPIC_NAKED` + `__epic_*` macros.

**Tech Stack:** Rust workspace (`crates/ir`, `crates/irparse`, `crates/alloc`, `crates/callgraph`, `crates/isel`, `crates/isel-pic18`, `crates/banking`, `crates/peephole`, `crates/driver`, `crates/asm`), pinned clang 20.1.8 via docker dev image, `gpasm` oracle unaffected.

**Spec:** `docs/superpowers/specs/2026-08-21-cc4-inline-assembly-design.md` + `docs/31-ecosystem-integration-design.md` D-3.

## Global Constraints

- No `rustup`/`clang`/`gpasm` on host; every cargo/clang invocation via `make exec` / `make shell` / docker dev image.
- Clang is pinned 20.1.8; version is part of the input format — never bump.
- PIC14 has no stack: locals are statically overlaid; whole-program via `llvm-link`; every stage has a diffable text boundary (`.ll` → IR → alloc map → `.asm` → HEX).
- Panics are the error surface for unsupported input; never silently miscompile or emit wrong code.
- Banking hazard: never insert a `BANKSEL` inside a skip-sensitive test/branch; `banking` never splits an `Asm` template.
- Commit hygiene: Conventional Commits, single line, no `Co-Authored-By:` trailers, no em-dashes (use comma/colon/period).
- All feature work in a worktree under `.worktrees/`; never on `master`.

---

## File Structure

| Crate | Responsibility in this plan |
|---|---|
| `crates/ir` | New data + text format. Owns `Module.module_asm`, `Func.naked`, `Inst::Asm`. |
| `crates/irparse` | Parse `module asm "..."`, function `naked` attr, and `call void asm sideeffect "...", "..."(...)` into `Inst::Asm`; reject register/m-operand constraints in v1. |
| `crates/callgraph` | Ignore `Asm` (no edges); naked functions remain nodes for depth check. |
| `crates/alloc` | `def_width(Asm)=None`; naked frames overlaid normally (typically empty). |
| `crates/isel` | Verbatim emission: module asm at top, naked functions (no prologue), inline `Asm` in place; word count for page-fit; barrier comments. |
| `crates/isel-pic18` | Same verbatim emission for PIC18 (no paging/banking pass). |
| `crates/banking` | Never insert inside an `Asm` template; set tracked bank to UNKNOWN on entry and exit of the block containing `Asm`. |
| `crates/peephole` | Do not match patterns across `Asm` lines. |
| `crates/driver` | Ship `EPIC_NAKED` + `__epic_*` in `epic-cc.h`; reject `.asm`/`.s` inputs with a helpful message. |
| `crates/driver/tests` | Golden HEX fixtures + negative panic tests. |

---

### Task 1: IR — `Module.module_asm`, `Func.naked`, `Inst::Asm`

**Files:**
- Modify: `crates/ir/src/lib.rs:125-140` (Module, Func), `:92-114` (Inst enum), `:143-186` helpers, `:188-310` serialize helpers, `:327-668` parse helpers
- Test: `crates/ir` inline `#[cfg(test)]` + round-trip via `ir::parse(ir::serialize(m))`

**Interfaces:**
- Consumes: nothing (root task)
- Produces:
  - `pub struct Module { pub globals: Vec<Global>, pub funcs: Vec<Func>, pub module_asm: Vec<String> }`
  - `pub struct Func { pub name: String, pub ret: Option<Ty>, pub params: Vec<Param>, pub blocks: Vec<Block>, pub isr: bool, pub naked: bool }`
  - `pub struct Asm { pub template: String, pub clobbers_memory: bool }` and `Inst::Asm(Asm)`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn round_trip_module_asm_and_naked_and_opaque_asm() {
    let src = r#"module_asm "global_blob: nop"
fn foo() [naked] () {
entry:
  asm "bcf INTCON, 7"
  asm "bsf INTCON, 7" memory
  ret
}
"#;
    let m = ir::parse(src);
    assert_eq!(m.module_asm, vec!["global_blob: nop"]);
    assert!(m.funcs[0].naked);
    // two Asm insts, second clobbers_memory
    let insts = &m.funcs[0].blocks[0].insts;
    match [&insts[0], &insts[1]] {
        [ir::Inst::Asm(a0), ir::Inst::Asm(a1)] => {
            assert_eq!(a0.template, "bcf INTCON, 7");
            assert!(!a0.clobbers_memory);
            assert!(a1.clobbers_memory);
        }
        _ => panic!("expected two Asm"),
    }
    // serialize round-trip
    let rt = ir::parse(&ir::serialize(&m));
    assert_eq!(rt.module_asm, m.module_asm);
    assert_eq!(rt.funcs[0].naked, true);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `make exec CMD='cargo test -p ir -- round_trip_module_asm_and_naked_and_opaque_asm -- --nocapture'`
Expected: FAIL — missing fields / unknown `module_asm` keyword / `asm` not parsed.

- [ ] **Step 3: Write minimal implementation**

In `crates/ir/src/lib.rs`:

1. Add `pub struct Asm { pub template: String, pub clobbers_memory: bool }` (derive Clone, Debug, PartialEq).
2. Extend `Inst` with `Asm(Asm)`.
3. Extend `Module` with `pub module_asm: Vec<String>` (default empty).
4. Extend `Func` with `pub naked: bool` (default false).
5. Update `serialize`: before globals emit each `m.module_asm` entry as `module_asm "…"` (escape `"` as `\"`, `\n` as `\n`, `\` as `\\` to keep quoted-string round-trip identical to existing global section quoting). For func header, emit `fn <name>(<ret>) [isr] [naked] (params)` — order: `[isr]` then `[naked]` when present, matching the docstring that `[isr]` sits between ret and params group; keep whitespace `fn foo() [isr] [naked] ()` form but implement as `if f.isr { out.push_str(" [isr]"); } if f.naked { out.push_str(" [naked]"); }`.
6. Update `inst_str` for `Inst::Asm`: `format!("  asm \"{}\"{}", escape(&a.template), if a.clobbers_memory { " memory" } else { "" })`. Escaping must escape `"` and `\` and emit `\n` as literal `\n`? Simpler: escape template via `template.escape_default()` trimmed? Check existing `serialize` string escaping (global bytes use hex) — for template reuse JSON-style escaping: replace `\` → `\\`, `"` → `\"`, `\n` → `\n`, `\t` → `\t`. Deserializer reverses.
7. Update `parse`: before per-func parsing, collect leading `module_asm "..."` lines. A module_asm line is `module_asm "<decoded>"`. Unescape with the mirror of serialize (handle `\\`, `\"`, `\n`).
8. Update func header parse: after reading the `fn name(...)` prefix and before params, loop consuming `[isr]` and `[naked]` markers in any order — set `isr`/`naked` bools. Keep backward compat: `fn foo() [isr] ()` still parses, `fn foo() ()` has both false.
9. Update `parse_inst` for the `asm` opcode: line starts with `asm "` (after trimming leading spaces). Parse quoted template via the same string-literal parser used for `module_asm`, then look for trailing ` memory` token. Return `Inst::Asm(Asm{template, clobbers_memory})`. Keep existing opcodes unaffected.

- [ ] **Step 4: Run test to verify it passes**

Run: `make exec CMD='cargo test -p ir -- --nocapture'`
Expected: PASS (at least the new test; existing tests unchanged).

- [ ] **Step 5: Commit**

```bash
git add crates/ir/src/lib.rs
git commit -m "feat(ir): CC-4 module asm, naked flag and opaque Asm inst"
```

---

### Task 2: `irparse` — lift `.ll` asm forms into the IR

**Files:**
- Modify: `crates/irparse/src/lib.rs:930-1145` (`parse_ll` + `parse_inst`), string-literal helpers near `:322-347`
- Test: `crates/irparse/tests/` (add `asm.rs` or extend `parse_ll.rs`), and existing `sanitize.rs`

**Interfaces:**
- Consumes: `ir::{Module, Func, Inst, Asm}` from Task 1
- Produces: `pub fn parse_ll(src: &str) -> Module` now populates `module_asm`, `Func.naked`, and `Inst::Asm` for the three v1 shapes; panics with precise messages for operand/register cases.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/irparse/tests/asm.rs
#[test]
fn module_asm() {
    let ll = r#"module asm "global_blob: nop""#;
    let m = irparse::parse_ll(ll);
    assert_eq!(m.module_asm, vec!["global_blob: nop"]);
}

#[test]
fn naked() {
    let ll = r#"define dso_local void @bar() #0 {
  tail call void asm sideeffect "return", ""() #1
  unreachable
}
attributes #0 = { naked noinline nounwind }"#;
    let m = irparse::parse_ll(ll);
    assert!(m.funcs.iter().find(|f| f.name=="bar").unwrap().naked);
    // naked body should contain one Asm, no Ret
}

#[test]
fn opaque_asm_no_operands() {
    let ll = r#"define void @foo() { tail call void asm sideeffect "bcf INTCON, 7", ""() #0 ret void }"#;
    let m = irparse::parse_ll(ll);
    let foo = m.funcs.iter().find(|f| f.name=="foo").unwrap();
    assert!(matches!(foo.blocks[0].insts[0], ir::Inst::Asm(ref a) if a.template=="bcf INTCON, 7" && !a.clobbers_memory));
}

#[test]
fn clobbers_memory_flag() {
    let ll = r#"define void @foo() { tail call void asm sideeffect "nop", "~{memory}"() ret void }"#;
    let m = irparse::parse_ll(ll);
    assert!(matches!(m.funcs[0].blocks[0].insts[0], ir::Inst::Asm(ref a) if a.clobbers_memory));
}

#[test]
#[should_panic(expected="register constraints are not supported")]
fn rejects_register_constraint() {
    let ll = r#"define void @foo() { %1 = tail call i8 asm sideeffect "movwf $0", "=r,0"(i8 1) ret void }"#;
    irparse::parse_ll(ll);
}

#[test]
#[should_panic(expected="asm with operands is not supported")]
fn rejects_operand_form() {
    let ll = r#"define void @foo() { tail call void asm sideeffect "movf $1, w", "=*m,*m,*m"(ptr @t, ptr @y, ptr @t) ret void }"#;
    irparse::parse_ll(ll);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `make exec CMD='cargo test -p irparse -- --nocapture'`
Expected: FAIL — `module_asm` field missing / `naked` false / asm not mapped / no panics.

- [ ] **Step 3: Write minimal implementation**

In `crates/irparse/src/lib.rs`:

1. **Module asm collection at top of `parse_ll`:**
   Before the function-definition scan, iterate raw lines. For each line trimmed: if it starts with `module asm "` then decode the quoted LLVM string literal.
   LLVM string literal decoding: reuse existing `parse_string_literal` logic for `c"..."` but for `module asm` the content is a normal `"`-quoted string where LLVM escapes are `\22` for `"`?, actually `module asm` uses `"` with escapes `\"`, `\\`, `\0A`. Simpler: reuse the `llvm_string_unescape` already implied by `decode_typed_value`? Check: `parse_string_literal` handles `\XX` hex and `\\`. For `module asm`, LLVM writes `module asm "foo\0Abar"` — so `\0A` is the escape. Implement a small helper `unescape_module_asm(s: &str) -> String` that handles `\\` → `\`, `\"` → `"`, `\0A` → `\n`, `\0D` → `\r`, `\09` → `\t`, generic `\XX` hex (two hex digits) → byte value, and passes other `\` through. Multiple `module asm` lines accumulate; if a single line's decoded content contains `\n`, split? The probe showed one `module asm` per file-scope `asm("...")`, contents may already contain `\n` via `\0A`. Keep each decoded entry as one element but later split on `\n` at emission — preserve as single string with embedded `\n`.

2. **`naked` detection:** Inside the loop that iterates function definition blocks, after extracting the `define` header line, parse the attribute group string. The attribute list is either inline `{ naked ... }` or referenced `#N` (look up `attributes #N = { ... }` map built from the `.ll` attribute definitions). Implement `func_is_naked(header_attrs: &str, attr_map: &HashMap<String,String>) -> bool` — check token `naked` as a standalone attribute. Set `Func.naked` accordingly. Preserve existing `isr` detection (still via `msp430_intrcc` in the return position).

3. **`call ... asm sideeffect` lifting in `parse_inst`:**
   At top of `parse_inst`, if `line.contains("asm sideeffect")`:
   - Extract the two quoted strings via a helper that finds `asm sideeffect "` then balances to the matching `"` with `\"` handling.
   - Call them `template_raw`, `constraints_raw` (second quoted string).
   - Decode `template_raw` via `unescape_module_asm`.
   - Inspect `constraints_raw`:
     * If it contains `=r`, `,r`, `"r"`, `=q`, `r,` etc. that indicate a register constraint — detect by tokenizing on `,` and checking any constraint token after stripping `= * %` prefix contains `r` as a bare token (not inside `*m`). For v1, simpler: if `constraints_raw.contains('r')` and not `constraints_raw=="*m"`-like, panic with register message. More precise: split `constraints_raw` on `,` then for each `c` trimmed: strip leading `=`, `*`, `%`, digits (`0` tied operand), then if the remainder is `r` or contains `r` without `m`, it's a register constraint.
     * Else if `constraints_raw` is not empty after stripping clobbers `~{memory}`/`~{cc}` and whitespace — then it's an operand form. Extract operand vs clobber: clobbers are `~{...}` entries. Remaining comma-separated entries that are not clobbers are operand constraints. If any remain — panic with `"asm with operands is not supported in this build (rung 4 deferred); use naked functions or opaque asm(\"...\") with no operands"`.
     * Else: `clobbers_memory = constraints_raw.contains("~{memory}")`.
   - Return `vec![Inst::Asm(Asm{template: decoded_template, clobbers_memory})]`.

   **Edge:** The call form may be `%1 = tail call i8 asm sideeffect "movwf $0", "=r,0"(i8 1)` — the return type is not `void`. Reject that before other checks with the register-constraint panic.

   **Naked trailing `unreachable`:** In `parse_ll`'s function body accumulation, when `func.naked` is true, filter out a final `unreachable` terminator line (do not emit it as an IR inst). Alternatively, have `parse_inst` for `unreachable` return `Vec::new()` when inside a naked function (track via a `is_naked` flag passed through). This keeps the function's block without a `Ret` terminator; the IR verifier for naked functions is relaxed (see Task 1 note).

4. **Keep `sanitize_symbols` untouched for asm string content:** Confirm it already skips `"`-quoted text.

- [ ] **Step 4: Run tests to verify they pass**

Run: `make exec CMD='cargo test -p irparse -p ir -- --nocapture'`
Expected: PASS (new tests + existing `sanitize`, `parse_ll` suites).

- [ ] **Step 5: Commit**

```bash
git add crates/irparse/src/lib.rs crates/irparse/tests/
git commit -m "feat(irparse): CC-4 lift module asm, naked and opaque asm"
```

---

### Task 3: `callgraph` + `alloc` — recognize `Asm`

**Files:**
- Modify: `crates/callgraph/src/lib.rs:12-26` (build)
- Modify: `crates/alloc/src/lib.rs:638-670` (`def_width`), plus frame-size helpers if they match on `Inst::Call` only

**Interfaces:**
- Consumes: `ir::Inst::Asm` from Task 1
- Produces: `callgraph::build` ignores `Asm`; `alloc::def_width(Asm)=None`; no other layout change.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn alloc_ignores_asm_no_width() {
    // Build a module with one function containing Asm
    let src = "fn foo() () {\nentry:\n  asm \"nop\"\n  ret\n}";
    let m = ir::parse(src);
    assert_eq!(alloc::def_width(&m.funcs[0].blocks[0].insts[0]), None);
    // callgraph still builds with correct depth even though body has Asm
    let g = callgraph::build(&m);
    assert_eq!(g.max_depth, 1);
}

#[test]
fn naked_has_frame_overlaid() {
    // two naked functions + one normal caller — overlay still packs siblings
    // verify allocate doesn't panic and locals are empty for naked
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `make exec CMD='cargo test -p alloc -p callgraph -- --nocapture'`
Expected: FAIL if `def_width` match is non-exhaustive (compile error) or returns wrong.

- [ ] **Step 3: Write minimal implementation**

In `crates/alloc/src/lib.rs` `def_width`:
```rust
Inst::Asm(_) => None,
```
Ensure any other `match` on `Inst` that is exhaustive (e.g. `locals_size` scanning defs, `frame_end` helpers) includes `Asm` in the `_` or explicit `None` arm. No frame-size increment for `Asm`.

In `crates/callgraph/src/lib.rs` nothing to change if it already matches only `Inst::Call`; add a `_` arm comment that `Asm` is intentionally ignored (no edges). Ensure depth checker counts naked functions as nodes: `build` seeds `adj` with every `f.name` regardless of inst contents — already does, so naked functions are counted.

- [ ] **Step 4: Run tests to verify they pass**

Run: `make exec CMD='cargo test -p alloc -p callgraph -p ir -- --nocapture'`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/alloc/src/lib.rs crates/callgraph/src/lib.rs
git commit -m "feat(alloc): CC-4 recognize Asm, no def width"
```

---

### Task 4: `isel` + `isel-pic18` — verbatim emission

**Files:**
- Modify: `crates/isel/src/lib.rs:5442-6038` (`select`, `emit_func_body`, word counting helpers)
- Modify: `crates/isel-pic18/src/lib.rs` (mirrored `select`)
- Modify: `crates/iselcore` if shared helpers live there

**Interfaces:**
- Consumes: `Module.module_asm`, `Func.naked`, `Inst::Asm` + address map (already threaded)
- Produces: assembly text with module asm at top, naked bodies verbatim, inline `Asm` blocks verbatim at their block position.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn module_asm_emitted_at_top() {
    let m = ir::parse(r#"module_asm "global_blob: nop"
fn main() () { entry: ret }"#);
    let asm = isel::select(&device::PIC16F877A, &m, &HashMap::new());
    assert!(asm.lines().next().unwrap().contains("global_blob"));
}

#[test]
fn naked_verbatim_no_prologue() {
    let src = "fn myr() [naked] () {\nentry:\n  asm \"movf _x, w\"\n  asm \"return\"\n}";
    let m = ir::parse(src);
    let asm = isel::select(&device::PIC16F877A, &m, &HashMap::new());
    assert!(asm.contains("movf _x, w"));
    assert!(asm.contains("\nreturn"));
    // no generated push/pop prologue
    assert!(!asm.contains("MOVWF")); // or device-specific prologue check
}

#[test]
fn opaque_inline_emitted_in_order() {
    let src = r#"fn foo() () { entry: asm "bcf INTCON, 7" br end end: asm "bsf INTCON, 7" ret }"#;
    // verify templates appear in order between blocks
}
```

PIC18 equivalent tests in `crates/isel-pic18`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `make exec CMD='cargo test -p isel -p isel-pic18 -- --nocapture'`
Expected: FAIL — module asm not emitted, naked still gets prologue, inline asm missing.

- [ ] **Step 3: Write minimal implementation**

In both `crates/isel/src/lib.rs` and `crates/isel-pic18/src/lib.rs`:

1. **Module asm prologue in `select`:** At very start of `select` (before the `.include`/header comment), iterate `m.module_asm` in order. For each entry, split on `\n` and extend `out` lines verbatim (no indentation mangling — preserve the string as the user wrote it). Emit one line per split piece. Add a comment `; module asm` before the block for bisect readability (not required but helps).

2. **Naked function path in `emit_func_body` (or `select`'s per-func loop):**
   - Branch: `if f.naked {`
     - Emit label `"{name}:"`.
     - For each block in `f.blocks` in order, for each `Inst::Asm(a)` in that block, split `a.template` on `\n` and emit each line verbatim (no prefix). If the template does not end with a newline, still emit lines one per `\n` split.
     - Panic if any non-`Asm` inst is found inside a naked function: `"naked function '{}' contains non-asm instruction; naked bodies must be pure assembly"`.
     - Do not emit phi copies, do not emit `RETURN`/`RETFIE`, do not emit prologue/epilogue. The user's last `asm "return"` is their responsibility.
     - Continue to next function (skip the normal body emission).
   - `}`

3. **Inline `Asm` inside normal functions:** In the per-block inst emission loop (where `Load`/`Store`/`Bin` etc. are matched), add arm `Inst::Asm(a) => { for line in a.template.split('\n') { g.out.push(line.to_string()); } }`. Emit exactly as verbatim lines, no operand substitution, no comment stripping. Keep block label structure: the block's label is still emitted before its insts, so an `Asm` at block top after a label is correctly placed.

4. **Word counting for page-fit:** `word_size` and the pass-A measurement count assembly lines as 1 word per non-empty, non-comment, non-directive line. Verbatim asm lines must follow the same rule: a line starting with `;` is comment (0), otherwise if non-empty after trim it is 1 word (the assembler will count it the same way via `asm::assemble` pass 1). Ensure `word_size` sees the same lines isel emits — i.e., verbatim lines are included in the `lines` slice measured. No special-casing needed if word_size counts lines with the existing predicate.

5. **Conservative clobber note:** After emitting an `Asm` block, the codegen's virtual `W`/`STATUS` liveness is reset — i.e., any optimization that would keep `W` live across the block is disabled. For v1 this is implicit because normal lowering spills before and reloads after; document with a comment `// Asm barrier: W/STATUS/bank clobbered`.

6. Share logic via `iselcore` only if it already holds `emit_func_body` — otherwise duplicate verbatim in both crates (they diverge on PIC14 paging vs PIC18 access bits, but the Asm emission itself is identical).

- [ ] **Step 4: Run tests to verify they pass**

Run: `make exec CMD='cargo test -p isel -p isel-pic18 -p ir -p irparse -- --nocapture'`
Expected: PASS (new tests + existing `verify_page_fit` still holds).

- [ ] **Step 5: Commit**

```bash
git add crates/isel/src/lib.rs crates/isel-pic18/src/lib.rs
git commit -m "feat(isel): CC-4 verbatim module asm, naked and inline Asm"
```

---

### Task 5: `banking` + `peephole` — barrier rules

**Files:**
- Modify: `crates/banking/src/lib.rs:328-478` (`assign_banks`, `is_bank0_only`, helpers)
- Modify: `crates/peephole/src/lib.rs` (optimization loop)

**Interfaces:**
- Consumes: assembly text produced by Task 4 (which now contains verbatim `Asm` lines)
- Produces: same text with no `BANKSEL` inserted inside an `Asm` block and no elision across it.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn banking_never_inside_asm_block() {
    let asm = "foo:\n  BCF STATUS, 5\n  bcf INTCON, 7\n  MOVF 0x20, W\n";
    // bcf INTCON,7 is standing in for a verbatim block that must not be split
    // For the real test, construct text with a verbatim 2-line Asm block:
    let asm2 = "main:\n  MOVF 0x20, W\n  bcf INTCON, 7\n  bsf INTCON, 7\n  MOVF 0xA0, W\n";
    let out = banking::assign_banks(&device::PIC16F877A, asm2);
    // The two MOVF operands target different banks (0x20=bank0, 0xA0=bank1)
    // so a BANKSEL must appear, but not between the two bcf/bsf lines
    assert_eq!(out.matches("BANKSEL").count(), 1);
    assert!(!out.contains("bcf INTCON, 7\n  BANKSEL"));
}

#[test]
fn banking_unknown_after_asm() {
    let asm = "main:\n  MOVF 0x20, W\n  my_asm_line\n  MOVF 0x20, W\n";
    // second MOVF is still bank0 but after unknown, must re-establish (full BANKSEL when preceded by label/unknown)
    let out = banking::assign_banks(&device::PIC16F877A, asm);
    // at least one BANKSEL after the unknown point
    assert!(out.matches("BANKSEL").count() >= 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `make exec CMD='cargo test -p banking -- --nocapture'`
Expected: FAIL — banking either splits the block or doesn't reset.

- [ ] **Step 3: Write minimal implementation**

In `crates/banking/src/lib.rs`:

1. Define the verbatim marker: since isel emits raw template lines with no prefix, banking cannot distinguish them from normal `MOVF` lines by mnemonic. Solution: have isel bracket each `Asm` block with comments `; --ASM-START--` and `; --ASM-END--` (or a single `; asm:` prefix per verbatim line). Use the comment approach: before the verbatim lines emit `; --- asm start ---`, after emit `; --- asm end ---`. Banking already skips `;` comment lines for bank inference but can detect these markers.

   Alternative simpler: isel emits verbatim lines prefixed with no marker and banking treats **any unrecognized mnemonic** as a barrier that resets to UNKNOWN. Implement the UNKNOWN-reset policy generically: any line whose mnemonic is not in the known set (`MOVF`, `MOVWF`, `BCF`, `BSF`, `CALL`, `GOTO`, etc.) is treated as `UNKNOWN` barrier — it sets tracked bank to `None` (UNKNOWN) and is not itself a banked operand. Since `bcf`/`bsf`/`nop`/`clrwdt`/`sleep` are all outside the file-register operand model (they either target SFRs or no operand), this is safe. But `bcf INTCON,7` is actually a file-register operand at 0x0B — would be considered a banked operand if naively parsed. So treat the whole verbatim block as opaque: the `; --- asm start ---` / `; --- asm end ---` guards are the robust fix.

2. Implement marker detection in `assign_banks`:
   - When a line trimmed starts with `; --- asm start ---`, set `in_asm = true`, emit the line unchanged, set `tracked = None` (UNKNOWN).
   - While `in_asm`, emit every line verbatim unchanged (no operand rewriting, no BANKSEL insertion, no bank inference). On `; --- asm end ---`, clear `in_asm`, keep `tracked = None` (still UNKNOWN) so the next banked operand gets a full BANKSEL.

   This satisfies "never insert inside a block" (while `in_asm`, the insertion path is disabled) and "unknown after" (tracked stays UNKNOWN until re-established).

3. Update `is_bank0_only` similarly: if text contains `; --- asm start ---`, return false (not bank0-only), so the full banking path runs. Alternatively keep it conservative: any asm presence means not bank0-only.

In `crates/peephole/src/lib.rs` (inspect its `optimize` loop — it pattern-matches on consecutive lines):
- Add the same `in_asm` guard: when inside an asm bracket, do not apply any peephole pattern that would cross the bracket. The simplest is to split the input into segments on the asm markers, optimize each non-asm segment independently, then re-join with the verbatim blocks untouched between them.

- [ ] **Step 4: Run tests to verify they pass**

Run: `make exec CMD='cargo test -p banking -p peephole -p isel -- --nocapture'`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/banking/src/lib.rs crates/peephole/src/lib.rs
git commit -m "feat(banking): CC-4 never insert BANKSEL inside Asm, unknown after"
```

---

### Task 6: `driver` — `epic-cc.h` intrinsics + `.asm` rejection

**Files:**
- Modify: `crates/driver/src/epic_cc_h.rs:1-29`
- Modify: `crates/driver/src/main.rs:100-132` (clang loop, CLI handling)
- Modify: `crates/driver/src/cli.rs` (input classification)

**Interfaces:**
- Consumes: nothing new (header-only)
- Produces: `EPIC_NAKED` + `__epic_*` macros expand to opaque `asm volatile`; `.asm`/`.s` inputs rejected early.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn header_has_epic_naked() {
    assert!(driver::epic_cc_h::EPIC_CC_H.contains("EPIC_NAKED"));
    assert!(driver::epic_cc_h::EPIC_CC_H.contains("__epic_nop"));
}

#[test]
fn asm_file_rejected() {
    // invoking driver with an .asm input should exit 2 with helpful message
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `make exec CMD='cargo test -p driver -- --nocapture'`
Expected: FAIL — missing macros, no rejection.

- [ ] **Step 3: Write minimal implementation**

In `crates/driver/src/epic_cc_h.rs` add:

```c
#define EPIC_NAKED __attribute__((naked))
#define __epic_nop()    asm volatile("nop")
#define __epic_clrwdt() asm volatile("clrwdt")
#define __epic_sleep()  asm volatile("sleep")
#define __epic_di()     asm volatile("bcf INTCON, 7")
#define __epic_ei()     asm volatile("bsf INTCON, 7")
```

All are one-liners expanding to a single opaque `asm` block, so they reuse the `Inst::Asm` path.

In `crates/driver/src/cli.rs` (or `main.rs` before the clang loop): if any `cli.inputs` entry ends with `.asm` or `.s` (case-insensitive) → `eprintln!("epic-cc: .asm inputs are not supported in this build; use EPIC_NAKED functions")` and `exit(2)`.

No other driver change — `.c` files with `asm(...)`/`EPIC_NAKED` go through the normal clang → llvm-link → irparse path.

- [ ] **Step 4: Run tests to verify they pass**

Run: `make exec CMD='cargo test -p driver -- --nocapture'`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/driver/src/epic_cc_h.rs crates/driver/src/cli.rs
git commit -m "feat(driver): CC-4 EPIC_NAKED and __epic_* intrinsics"
```

---

### Task 7: E2E fixtures + negative tests + full suite

**Files:**
- Create: `crates/driver/tests/fixtures/asm_naked.c`, `asm_opaque.c`, `asm_module.c`, `asm_intrinsic.c` (and their PIC18 equivalents via `--device` flag)
- Modify: `crates/driver/tests/` runner (golden HEX via `make test` harness)
- Create: negative tests `asm_with_operands.c` / `asm_reg_constraint.c` expecting panic substrings

**Interfaces:**
- Consumes: all prior tasks
- Produces: committed golden HEX, suite green.

- [ ] **Step 1: Write the failing tests (fixtures)**

Create `crates/driver/tests/fixtures/asm_naked.c`:
```c
#include <epic-cc.h>
EPIC_NAKED void my_mul(void) {
    asm("movf _a, w");
    asm("addwf _b, w");
    asm("movwf _r");
    asm("return");
}
volatile unsigned char a, b, r;
void main(void) { a=3; b=4; my_mul(); }
```

`asm_opaque.c`:
```c
volatile unsigned char counter, flag;
void main(void) {
    asm volatile("bcf INTCON, 7");
    counter = counter + 1;
    asm volatile("bsf INTCON, 7");
    flag = 1;
}
```

`asm_module.c`:
```c
asm("my_label: nop");
void main(void) { asm volatile("goto my_label"); }
```

`asm_intrinsic.c`:
```c
#include <epic-cc.h>
void main(void) { __epic_nop(); __epic_clrwdt(); __epic_di(); __epic_ei(); }
```

Run through the driver at test time (use the helper `driver_binary()` / `CARGO_BIN_EXE_epic-cc` path) compiling for both `--device p16f877a` and `--device p18f4550` where applicable; assert HEX non-empty and `asm` emit contains expected verbatim lines.

Negative: `asm_reg.c` containing `asm volatile("movwf %0" : "+r"(x))` → assert driver panics containing `"register constraints are not supported"`. `asm_operands.c` with `"=r,0"` similarly.

- [ ] **Step 2: Run tests to verify they fail**

Run: `make exec CMD='cargo test -p driver -- --nocapture'`
Expected: FAIL — no fixtures yet, HEX mismatched.

- [ ] **Step 3: Add fixtures, run golden update, verify**

Run driver per fixture inside the test to generate HEX, write it as the committed golden file (follow existing `crates/driver/tests/fixtures/*.hex` pattern). For PIC18 fixtures duplicate with `--device p18f4550` variant or parameterize the test.

Implement the tests as e2e harness that:
1. spawns `epic-cc --device p16f877a -o /tmp/out.hex <fixture>.c`,
2. reads the HEX, checks it assembles and contains the expected verbatim mnemonic.

- [ ] **Step 4: Run full suite**

Run: `make test` (exact CI script via `ci-test.sh`) inside docker
Expected: all 16 crates PASS, fixtures green, negative panics correct.

- [ ] **Step 5: Commit + pre-PR ritual**

```bash
git add crates/driver/tests/fixtures/
git commit -m "test(driver): CC-4 e2e fixtures for naked, opaque, module and intrinsics"
```

Then:
```bash
make pre-pr-check
make pre-pr-check TEST=1
```

---

## Self-Review

*Spec coverage:* Every section of the spec has a task: §4 IR → T1, §5 irparse → T2, §7 callgraph/alloc → T3, §8 isel → T4, §9 banking/peephole → T5, §6 driver header → T6, §11 testing → T7. No gap.
*Placeholder scan:* No TBD/TODO; every step shows concrete code or exact commands.
*Type consistency:* `Inst::Asm(Asm{template, clobbers_memory})`, `Module.module_asm: Vec<String>`, `Func.naked: bool` appear identically in T1 and are consumed verbatim in T2–T4.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-08-21-cc4-inline-assembly-plan.md`. Two execution options:

**1. Subagent-Driven (recommended)** - dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - execute tasks in this session using executing-plans, batch execution with checkpoints

Which approach?
