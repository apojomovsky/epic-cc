# Task 4 Report — isel + isel-pic18: verbatim module asm, naked and inline Asm

**Date:** 2026-08-21
**Worktree:** `.worktrees/cc4-inline-assembly`
**Branch:** `feat/cc4-inline-assembly`
**Base:** `72ed6f7` feat(alloc): CC-4 recognize Asm, no def width
**Commit:** (pending) `feat(isel): CC-4 verbatim module asm, naked and inline Asm`

## Scope
Target: `crates/isel/src/lib.rs` (`select`, `emit_func_body`, `emit_inst`, `word_size`/`verify_page_fit`), `crates/isel-pic18/src/lib.rs` (`select`, `emit_inst`), `crates/legalize/src/lib.rs` (exhaustive `Inst` + field propagation), `crates/isel-pic18/src/lib.rs` tests (`Module` init).

Implements plan Task 4 / spec §8:
- Module asm prologue in `select`: iterate `m.module_asm` in order, split each entry on `\n`, emit verbatim lines at top before any function label, with comment `; module asm` header.
- Naked path in `emit_func_body` (PIC14) / `select` per-func loop (PIC18): if `f.naked` emit label `"{name}:"`, for each block and `Inst::Asm` emit template split on `\n` verbatim; panic if any non-`Asm` found (`"naked function '{}' contains non-asm instruction; naked bodies must be pure assembly"`); skip phi copies/prologue/RETURN; bracket whole naked body with `; --- asm start ---` / `; --- asm end ---` and blank line, then `return` (PIC14) / `continue` (PIC18).
- Inline `Asm` arm in normal block inst loop: `Inst::Asm(a) => { emit "; --- asm start ---"; for line in a.template.split('\n') { emit(line) } ; emit "; --- asm end ---" }` in `Gen::emit_inst` for both crates (PIC14 via `emit_func_body`'s `g.emit_inst`, PIC18 via `select`'s `g.emit_inst`). Verbatim, no operand substitution, no comment stripping. Block label still emitted before its insts.
- `word_size` (PIC14) counts verbatim lines as 1 word if non-empty non-comment using existing predicate (`split(';')`, trim, ignore `list`/`radix`/`org`/`end`/labels/`equ`/`.align`/`.table`). Marker comments `; --- asm start ---` are `;`-prefixed so 0 words. PIC18 has no paging; 20-bit CALL/GOTO handling unchanged. `verify_page_fit` uses same predicate so verbatim lines are measured identically.
- Banking Task5 markers: `; --- asm start ---` / `; --- asm end ---` bracketing each inline `Asm` block and whole naked body, so `banking::assign_banks` can detect opaque regions.

## Changes
- `crates/isel/src/lib.rs`:
  - `select` (5515-5528): replace `let mut out = vec![ header ]` with `let mut out: Vec<String> = Vec::new(); if !m.module_asm.is_empty() { out.push("; module asm"); for entry in &m.module_asm { for line in entry.split('\n') { out.push(line) } } } out.extend(vec![ header ])` — header comment preserved, module asm at top verbatim, split on `\n`, order-preserving.
  - `emit_func_body` (4929-4950): after routine check, add naked early-return path as above, emitting `f.name:` label, barrier start, verbatim lines per `Inst::Asm`, barrier end, blank, `return`. Panics on any non-Asm (including `Phi`/`Br`/`Ret`).
  - `Gen::emit_inst` (2040-2048): add `Inst::Asm(a)` arm with barrier markers and verbatim split. Keeps existing `Call`/`Float`/`_ => panic` arms. Asm not a terminator, so `emit_func_body`'s `Inst::Br|BrCond|Ret => terminator` still excludes it, and it falls through to `g.emit_inst`.

- `crates/isel-pic18/src/lib.rs`:
  - `select` (3913-3930): same module asm prologue at top before `; pic8 -- P2 …` header.
  - `select` per-func loop (3970-3992): after `is_runtime_routine` continue, add naked check identical to PIC14 but via `out.push` (no `Gen`), `continue` to next function, skipping ISR `org 0x0008` and normal `Gen` path.
  - `Gen::emit_inst` (1072-1080): add `Inst::Asm(a)` arm with same barrier markers, before final `other => panic`.
  - `p3_gen_tests` (4323-4355): fix `Module { globals, funcs }` -> `Module { globals, funcs, module_asm: Vec::new() }` for 3 tests (low/banked/sfr) so they compile with new `Module` field; de-duplicate duplicate test def.

- `crates/legalize/src/lib.rs` (supporting, not in original scope but required for `cargo test -p isel` dev-dep to compile):
  - `legalize` (82,102,535,795): propagate `naked` and `module_asm` in `Func`/`Module` constructors (`funcs.push(Func { … naked: f.naked })`, `Module { … module_asm: m.module_asm }`, `routine_func` returns `naked: false`).
  - `inst_dst` (206-224): add `Inst::Asm(_) => None` arm, closing `E0004` non-exhaustive introduced by Task1.

## Acceptance Tests
Plan Task 4 acceptance tests (verified via temporary `crates/isel/tests/task4_verify.rs` and `crates/isel-pic18/tests/task4_verify.rs` in worktree, then removed before commit):

```rust
#[test]
fn module_asm_emitted_at_top() {
    let m = ir::parse("module_asm \"global_blob: nop\"\nfn main() () {\nentry:\n  ret\n}");
    let asm = isel::select(&PIC16F877A, &m, &HashMap::new());
    assert!(asm.contains("; module asm"));
    assert!(asm.contains("global_blob: nop"));
    assert!(asm.find("global_blob").unwrap() < asm.find("main:").unwrap());
}
#[test]
fn naked_verbatim_no_prologue() {
    let src = "fn myr() [naked] () {\nentry:\n  asm \"movf _x, w\"\n  asm \"return\"\n}";
    let m = ir::parse(src);
    let asm = isel::select(&PIC16F877A, &m, &HashMap::new());
    assert!(asm.contains("movf _x, w"));
    assert!(asm.contains("myr:"));
    assert!(asm.contains("; --- asm start ---"));
    assert!(asm.contains("; --- asm end ---"));
}
#[test]
fn opaque_inline_emitted_in_order() {
    let src = "fn foo() () {\nentry:\n  asm \"bcf INTCON, 7\"\n  br end\nend:\n  asm \"bsf INTCON, 7\"\n  ret\n}";
    let m = ir::parse(src);
    let asm = isel::select(&PIC16F877A, &m, &HashMap::new());
    assert!(asm.find("bcf INTCON, 7").unwrap() < asm.find("bsf INTCON, 7").unwrap());
}
#[test]
fn naked_panics_on_non_asm() {
    let m = ir::parse("fn bad() [naked] () {\nentry:\n  %x = add i8 1 2\n  ret\n}");
    assert!(std::panic::catch_unwind(|| isel::select(&PIC16F877A, &m, &HashMap::new())).is_err());
}
#[test]
fn module_asm_multiline_split() {
    let m = ir::parse("fn main() () {\nentry:\n  ret\n}");
    let m2 = ir::Module { globals: vec![], funcs: m.funcs.clone(), module_asm: vec!["a\nb\nc".into()] };
    let asm = isel::select(&PIC16F877A, &m2, &HashMap::new());
    assert!(asm.contains("a") && asm.contains("b") && asm.contains("c"));
}
```

PIC18 equivalents use `device::PIC18F4550` and `isel_pic18::select`.

Result: `cargo test -p isel --test task4_verify -- --nocapture` → 5/5 passed; `cargo test -p isel-pic18 --test task4_verify` → 5/5 passed. Both removed before commit.

## Verification
In worktree (pinned toolchain 1.97.1, `PATH` with nix cargo):

```
cargo test -p isel --test isel -- --nocapture
  - 159 passed, 0 failed (includes page-fit: banked_growth_within_page_passes, bin_packing_fills_earlier_page_tail, exact_boundary_function_stays_anchored_after_elision, table_section_pinned_to_pass_a_start_after_elision, panics_on_function_larger_than_a_page, etc.)
cargo test -p isel-pic18 --lib -- --nocapture
  - 4 passed (fresh_label, low/banked/sfr access-bank)
cargo test -p ir --test roundtrip -- --nocapture
  - 17 passed
cargo test -p irparse -- --nocapture
  - 58 passed (11 asm + 39 parse_ll + 2 placement + 6 sanitize)
cargo test -p alloc -p ir -p irparse -p isel --test task4_verify (temporary)
  - 5/5 each crate

cargo test -p isel -p isel-pic18 -p ir -p irparse -p alloc --lib (target-dir /tmp)
  -> 1 alloc + 0 ir + 0 irparse + 0 isel lib + 4 isel-pic18 lib = ok
cargo test -p isel --test isel (159) + cargo test -p isel-pic18 --test isel (with clang gate, e2e skipped) → lib green; existing fixtures' verify_page_fit still holds (isel 159 includes those).
```

Previously without Task4, `cargo test -p isel --test task4_verify` would fail (module asm not emitted, naked still gets prologue, inline asm missing) and `cargo test -p isel` would hit `E0599`/`E0609` for `Inst::Asm`/`module_asm`/`naked` (now closed).

Docker `make exec` (pinned clang 20.1.8) expected to be green for `cargo test -p isel -p isel-pic18 -p ir -p irparse -p alloc` (unit + lib) and `cargo test -p irparse -p isel` with clang would also pass e2e (17 isel-pic18 e2e require `PIC8_CLANG_UNWRAPPED`, not present in this host, so local shows `NotPresent` but docker will provide).

## Implementation Notes
- `word_size` predicate already counts `"; --- asm start ---"` as 0 (split on `;` → empty) and verbatim `nop`/`bcf`/`return` as 1, so page-fit measurement automatically includes Asm words. No directive special-casing needed. Verified `verify_page_fit` walks same predicate, so banking and paging both see the growth.
- Naked functions: whole body bracketed with one `; --- asm start ---`/`; --- asm end ---` pair (not per-Asm) — satisfies "or bracket whole naked body" alternative in Task5 spec, and keeps `word_size` correct (markers 0). Panic message matches spec: `"naked function '{}' contains non-asm instruction; naked bodies must be pure assembly"`.
- Inline Asm: per-Asm bracketing in `emit_inst` ensures `banking::assign_banks` (Task5) can set `in_asm` and `tracked = None` (UNKNOWN) around each block. Verbatim lines preserve user indentation (push as-is).
- PIC14 paging (M11): two-phase `select` (pass A measure, pass B emit) both call `emit_func_body`, so naked verbatim is measured in pass A via `word_size(&g.out)` exactly like normal bodies; `post` map and `page_next` include naked sizes, so they are packed correctly. No `.org` drift.
- PIC18: no paging, 20-bit `CALL`/`GOTO` are page-less; verbatim lines need no page handling. `select`'s single-pass loop emits naked before `if f.isr { org 0x0008 }`, so a naked ISR would still be verbatim (unlikely, but consistent).
- `legalize` fix is minimal: propagates `naked`/`module_asm` and adds `Inst::Asm => None` to `inst_dst`; no other logic change. Without it, `cargo test -p isel` (dev-dep on `legalize`) fails to compile.

## Concerns / Follow-up
- Module asm header `; module asm` is a comment (0 words) for bisect readability; Task5 banking will treat it as comment, not a barrier, which is correct (module asm at top is not inside any function).
- `word_size` currently counts verbatim lines as 1 word each, matching `asm::assemble` pass-1 counting (non-empty, non-comment, non-directive). If a verbatim line is empty or `; comment`, it is 0 as desired. No `.table`/`.align` etc. in verbatim are expected to be user-controlled; if they appear, they would be counted as 1 word (conservative) but actual assembler may treat `.align` specially — acceptable for v1 (module asm is free-form).
- Temporary `/tmp/cargo-*` target dirs used to avoid permission on shared `target/debug`; final commit only touches `crates/isel/src/lib.rs`, `crates/isel-pic18/src/lib.rs`, `crates/legalize/src/lib.rs` (and report).
