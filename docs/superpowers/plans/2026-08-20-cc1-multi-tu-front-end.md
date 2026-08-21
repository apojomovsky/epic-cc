# CC-1 Multi-TU Front End Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `epic-cc` compiles a multi-file C program: a conventional CLI takes a list of `.c` files plus `-I`/`-D`/`-o`/`--device`, runs clang once per file, merges the `.ll` files with `llvm-link`, and feeds the single merged module to the existing pipeline unchanged.

**Architecture:** Per [D-7](../../31-ecosystem-integration-design.md#d-7-translation-units-merge-through-llvm-link-out-of-process), `llvm-link` does the merge out of process, exactly as clang does the parse out of process. Nothing links libLLVM. Symbol names are sanitized as a text transform on the merged `.ll` before parsing, `wholeprog` stops being a pass-through and becomes the validator that catches what `llvm-link` lets through, and `irparse` onward is untouched.

**Tech Stack:** Rust 1.97.1, Cargo workspace, no external crates. Docker dev image (`make shell`, `make test`). clang 20.1.8 and `llvm-link` 20.1.8 from `/opt/clang/bin`, both resolved through `PIC8_CLANG_UNWRAPPED`.

**Spec:** [`docs/31-ecosystem-integration-design.md`](../../31-ecosystem-integration-design.md), decision D-7 and sub-project CC-1.

## Global Constraints

- **Zero external crate dependencies.** `Cargo.lock` contains only the 16 workspace crates. Do not add `clap`, `regex`, `anyhow`, or anything else. Argument parsing and text scanning are hand-rolled.
- **The PIC14 backend never regresses.** Every existing fixture in `crates/driver/tests/fixtures/` must still produce its committed `.hex`. `crates/isel`, `crates/alloc`, `crates/banking`, `crates/peephole` and `crates/asm` are not touched by this plan.
- **Panics are the error surface.** Unsupported or malformed input aborts with a precise message naming the symbol or flag involved. Never emit code for input you did not understand.
- **No em-dashes (U+2014)** in code, comments, docs or commit messages. Use a comma, a colon, or a new sentence. The commit-msg hook and `make pre-pr-check` both reject them.
- **Conventional Commits, single line, no trailers.** `feat(driver): ...`, `fix(irparse): ...`. No `Co-Authored-By`.
- **Everything runs in the docker dev image.** `make test CRATE=<crate>` for one crate, `make test` for the suite. Never install a toolchain on the host.
- **Comments carry the why, not the what.** A comment restating the line below it gets deleted in review.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/irparse/src/lib.rs` | Gains `sanitize_symbols`, a pure `.ll` text transform. No change to `parse_ll`. |
| `crates/irparse/tests/sanitize.rs` | **Create.** Unit tests for the transform. |
| `crates/wholeprog/src/lib.rs` | Stops being a pass-through: validates the entry point and that every call target is defined. |
| `crates/wholeprog/tests/validate.rs` | **Create.** Unit tests for both checks. |
| `crates/driver/src/cli.rs` | **Create.** Hand-rolled argument parsing into a `Cli` struct. Pure, no I/O. |
| `crates/driver/src/clang_discovery.rs` | Gains `resolve_llvm_link`, which finds `llvm-link` beside the resolved clang. |
| `crates/driver/src/main.rs` | Orchestration: parse args, clang per input, `llvm-link`, sanitize, existing pipeline. |
| `crates/driver/tests/multi_tu_e2e.rs` | **Create.** The acceptance test: three units, colliding statics, cross-unit global and call. |
| `crates/driver/tests/fixtures/multi_tu_*.c` | **Create.** The three translation units. |
| ~25 existing call sites | Migrated from positional output to `-o` plus `--device`. |

Task order is dependency order. Tasks 1 through 4 are independent of each other and each lands green on its own; Task 5 is the one that flips the CLI and must migrate every call site in the same commit or the suite breaks.

---

### Task 1: `irparse::sanitize_symbols` — DONE (e0ed377)

`llvm-link` renames colliding internal symbols by appending a dot and a number (`@helper` plus `@helper` becomes `@helper` and `@helper.3`). Our own assembler keys labels as plain strings and does not care, but the `gpasm` byte-for-byte cross-check oracle has identifier rules, and `--emit asm` output is user-facing. Rewrite the dots before parsing.

Doing it as a text transform on the merged `.ll`, rather than as a walk over the parsed IR, is deliberate: a name reaches the IR through `Func.name`, `Global.name`, `Call.func`, `Val::Global`, `GepBase::Global`, `Load.ptr` and `Store.ptr`, so an IR walk would have to stay exhaustive across all 20 `Inst` variants forever. One text pass over `@`-prefixed identifiers covers every one of them.

**Files:**
- Modify: `crates/irparse/src/lib.rs`
- Create: `crates/irparse/tests/sanitize.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub fn sanitize_symbols(ll: &str) -> String`. Task 5 calls it between `llvm-link` and `irparse::parse_ll`.

- [x] **Step 1: Write the failing tests**

Create `crates/irparse/tests/sanitize.rs`:

```rust
use irparse::sanitize_symbols;

#[test]
fn rewrites_dots_in_symbol_names() {
    let ll = "@scratch.4 = internal global i8 0\ndefine i8 @helper.3(i8 %0) {\n";
    let out = sanitize_symbols(ll);
    assert!(out.contains("@scratch_4 = internal global i8 0"));
    assert!(out.contains("define i8 @helper_3(i8 %0)"));
}

#[test]
fn leaves_undotted_symbols_alone() {
    let ll = "define void @main() {\n  call void @helper()\n";
    assert_eq!(sanitize_symbols(ll), ll);
}

#[test]
fn leaves_llvm_intrinsics_alone() {
    // irparse matches these by prefix (`llvm.memcpy.p0.p0`) and they never
    // become assembler labels, so their dots must survive.
    let ll = "  call void @llvm.memcpy.p0.p0.i16(ptr %1, ptr %2, i16 4, i1 false)\n";
    assert_eq!(sanitize_symbols(ll), ll);
}

#[test]
fn leaves_registers_and_metadata_alone() {
    // `%` registers are function-local and never collide across modules;
    // `!` metadata and float literals both contain dots that are not symbols.
    let ll = "  %1 = fadd float %0, 1.5, !tbaa !2\n";
    assert_eq!(sanitize_symbols(ll), ll);
}

#[test]
fn does_not_rewrite_inside_string_constants() {
    // A C string literal reaching the .ll as `c"..."` can contain an @ and a
    // dot; rewriting inside it would corrupt program data.
    let ll = "@s = private constant [14 x i8] c\"user@host.com\\00\"\n";
    let out = sanitize_symbols(ll);
    assert!(out.contains("c\"user@host.com\\00\""), "string constant was rewritten: {out}");
    assert!(out.starts_with("@s = "));
}

#[test]
#[should_panic(expected = "sanitize to @helper_3")]
fn panics_when_two_symbols_collide_after_sanitizing() {
    let ll = "define void @helper.3() {\ndefine void @helper_3() {\n";
    sanitize_symbols(ll);
}
```

- [x] **Step 2: Run the tests to verify they fail**

Run: `make test CRATE=irparse`
Expected: FAIL, `cannot find function 'sanitize_symbols' in crate 'irparse'`.

- [x] **Step 3: Implement the transform**

Append to `crates/irparse/src/lib.rs`:

```rust
/// Rewrite `.` to `_` inside LLVM symbol names (`@name`) so downstream labels
/// are portable to `gpasm`, which rejects dots in identifiers. `llvm-link`
/// produces such names when it renames colliding internal symbols.
///
/// Intrinsics (`@llvm.memcpy.p0.p0`) keep their dots: `parse_ll` matches them
/// by prefix and they never become labels. `%` registers are function-local
/// and cannot collide, so they are left alone. Text inside `"` quotes is
/// copied through untouched, because a C string constant reaches the `.ll` as
/// `c"..."` and may contain an `@`.
///
/// Panics if two distinct symbols sanitize to the same name.
pub fn sanitize_symbols(ll: &str) -> String {
    let b = ll.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    // sanitized name -> the original it came from, for collision detection.
    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut i = 0;
    while i < b.len() {
        // LLVM escapes a quote inside a string as `\22`, never `\"`, so the
        // next `"` is always the closing one.
        if b[i] == b'"' {
            out.push(b[i]);
            i += 1;
            while i < b.len() && b[i] != b'"' {
                out.push(b[i]);
                i += 1;
            }
            continue;
        }
        if b[i] != b'@' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut j = start;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'_' | b'.' | b'$')) {
            j += 1;
        }
        let name = &ll[start..j];
        out.push(b'@');
        // Every non-intrinsic name goes through the map, dotted or not, so a
        // pre-existing `helper_3` is seen before `helper.3` sanitizes onto it.
        if name.is_empty() || name.starts_with("llvm.") {
            out.extend_from_slice(name.as_bytes());
        } else {
            let clean = name.replace('.', "_");
            match seen.get(&clean) {
                Some(prev) if prev != name => panic!(
                    "irparse: symbols @{prev} and @{name} both sanitize to @{clean}"
                ),
                _ => {
                    seen.insert(clean.clone(), name.to_string());
                }
            }
            out.extend_from_slice(clean.as_bytes());
        }
        i = j;
    }
    String::from_utf8(out).expect("sanitize_symbols: input was valid UTF-8")
}
```

- [x] **Step 4: Run the tests to verify they pass**

Run: `make test CRATE=irparse`
Expected: PASS, all six tests, and every pre-existing `irparse` test still green.

Actual: PASS, 6/6 new, 39/39 pre-existing.

- [x] **Step 5: Commit** — `e0ed377`

---

### Task 2: `wholeprog` validates the merged module — DONE (ff235a6)

`llvm-link` does not fail on a `declare` it could not satisfy, it leaves it in place. Left alone that becomes a `CALL` to a label the assembler has never heard of, and the user gets a panic from `crates/asm` naming a symbol they never wrote. Catch it here, while the names are still theirs.

This needs **no IR format change**: every call target is already in the IR as `Inst::Call(Call { func, .. })`, exactly as `crates/callgraph/src/lib.rs:18-25` walks them.

**Files:**
- Modify: `crates/wholeprog/src/lib.rs`
- Create: `crates/wholeprog/tests/validate.rs`

**Interfaces:**
- Consumes: `ir::{Module, Inst}`.
- Produces: `pub fn merge(m: Module) -> Module`, same signature as today. Behaviour changes from pass-through to validating.

- [x] **Step 1: Write the failing tests**

Create `crates/wholeprog/tests/validate.rs`. The IR text syntax below was taken from
`ir::serialize` output on 2026-08-20, not invented: a call is `%1 = call i8 @helper(i8 3)`,
a function header is `fn main(void) ()`, and blocks are `  block 0:`.

```rust
use ir::parse;
use wholeprog::merge;

const RESOLVED: &str = "\
fn main(void) ()
  block 0:
    %1 = call i8 @helper(i8 3)
    ret void
fn helper(i8) (0=i8)
  block 0:
    ret i8 %0
";

#[test]
fn accepts_a_resolved_module() {
    let out = merge(parse(RESOLVED));
    assert_eq!(out.funcs.len(), 2);
}

#[test]
#[should_panic(expected = "undefined symbols: from_b")]
fn rejects_a_call_with_no_definition() {
    merge(parse("\
fn main(void) ()
  block 0:
    %1 = call i8 @from_b(i8 3)
    ret void
"));
}

#[test]
#[should_panic(expected = "undefined symbols: alpha, beta")]
fn lists_every_undefined_symbol_sorted() {
    // Called in the order beta, alpha; reported sorted, because a BTreeSet
    // makes the diagnostic stable across runs.
    merge(parse("\
fn main(void) ()
  block 0:
    %1 = call i8 @beta(i8 1)
    %2 = call i8 @alpha(i8 2)
    ret void
"));
}

#[test]
#[should_panic(expected = "exactly one `main`")]
fn rejects_a_module_with_no_main() {
    merge(parse("\
fn helper(i8) (0=i8)
  block 0:
    ret i8 %0
"));
}
```

- [x] **Step 2: Run the tests to verify they fail**

Run: `make test CRATE=wholeprog`
Expected: FAIL. `accepts_a_resolved_module` passes trivially (today's `merge` is a pass-through), the three `should_panic` tests fail with "did not panic".

Actual: matched exactly.

- [x] **Step 3: Implement the checks**

Replace `crates/wholeprog/src/lib.rs` entirely:

```rust
//! Whole-program validation for the PIC8 pipeline: N translation units have
//! already been merged into one `.ll` by `llvm-link` (see docs/31 D-7), so
//! this stage does not link. It checks what `llvm-link` lets through.

use ir::{Inst, Module};
use std::collections::BTreeSet;

/// Validate the merged module and hand it on unchanged.
///
/// Panics if the module has no functions, if it does not contain exactly one
/// `main`, or if any call target has no definition.
pub fn merge(m: Module) -> Module {
    assert!(!m.funcs.is_empty(), "wholeprog: no functions in module");
    check_entry(&m);
    check_calls_resolved(&m);
    m
}

fn check_entry(m: &Module) {
    let mains = m.funcs.iter().filter(|f| f.name == "main").count();
    assert_eq!(mains, 1, "wholeprog: expected exactly one `main`, found {mains}");
}

/// `llvm-link` leaves an unsatisfied `declare` in place rather than failing.
/// Downstream that becomes a CALL to a label the assembler never heard of, so
/// the error has to be raised here, while the names are still the user's.
fn check_calls_resolved(m: &Module) {
    let defined: BTreeSet<&str> = m.funcs.iter().map(|f| f.name.as_str()).collect();
    let mut missing: BTreeSet<&str> = BTreeSet::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Call(c) = inst {
                    if !defined.contains(c.func.as_str()) {
                        missing.insert(c.func.as_str());
                    }
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "wholeprog: undefined symbols: {}",
        missing.into_iter().collect::<Vec<_>>().join(", ")
    );
}
```

- [x] **Step 4: Run the crate tests**

Run: `make test CRATE=wholeprog`
Expected: PASS, all four.

Actual: PASS, 4/4 new, plus the pre-existing `tests/merge.rs` (2 tests) unaffected.

- [x] **Step 5: Run the FULL suite, because this changes a shared stage**

Run: `make test`
Expected: PASS. Every existing fixture defines `main` (verified 2026-08-20), and `crates/driver/tests/*_e2e.rs` plus `crates/isel-pic18/tests/e2e.rs` call `wholeprog::merge` directly on parsed fixtures. If any of them now panics on `main`, that fixture is the bug report: read it before weakening the check.

Actual: PASS, all 16 crates, exit 0, no failures.

- [x] **Step 6: Commit** — `ff235a6`

---

### Task 3: driver argument parsing — DONE (cf6be7c)

Hand-rolled, because the workspace has zero external dependencies and keeps them that way.

**Files:**
- Create: `crates/driver/src/cli.rs`
- Modify: `crates/driver/src/main.rs` (add `mod cli;` only, no wiring yet)

**Interfaces:**
- Consumes: nothing.
- Produces:

```rust
pub struct Cli {
    pub inputs: Vec<String>,
    pub output: String,
    pub includes: Vec<String>,
    pub defines: Vec<String>,
    pub device: String,
    pub emit: Emit,
    pub save_temps: Option<String>,
    pub verbose: bool,
}
pub enum Emit { Ll, Ir, Asm, Hex }
pub fn parse_args(argv: &[String]) -> Result<Cli, String>;
```

`parse_args` takes the argument list **without** `argv[0]`. Task 5 calls it.

- [x] **Step 1: Write the failing tests**

Create `crates/driver/tests/cli.rs`:

```rust
use driver::cli::{parse_args, Emit};

fn args(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

#[test]
fn parses_a_minimal_invocation() {
    let c = parse_args(&args(&["a.c", "--device", "p16f877a"])).unwrap();
    assert_eq!(c.inputs, vec!["a.c"]);
    assert_eq!(c.output, "a.hex");
    assert_eq!(c.device, "p16f877a");
    assert!(matches!(c.emit, Emit::Hex));
}

#[test]
fn collects_multiple_inputs_includes_and_defines() {
    let c = parse_args(&args(&[
        "a.c", "b.c", "-I", "inc", "-I", "inc2", "-D", "F=1", "-D", "G",
        "-o", "out.hex", "--device", "p18f4550",
    ]))
    .unwrap();
    assert_eq!(c.inputs, vec!["a.c", "b.c"]);
    assert_eq!(c.includes, vec!["inc", "inc2"]);
    assert_eq!(c.defines, vec!["F=1", "G"]);
    assert_eq!(c.output, "out.hex");
    assert_eq!(c.device, "p18f4550");
}

#[test]
fn accepts_attached_short_flag_forms() {
    let c = parse_args(&args(&["a.c", "-Iinc", "-DF=1", "-oout.hex", "--device", "p16f877a"])).unwrap();
    assert_eq!(c.includes, vec!["inc"]);
    assert_eq!(c.defines, vec!["F=1"]);
    assert_eq!(c.output, "out.hex");
}

#[test]
fn parses_emit_stages() {
    for (s, want) in [("ll", Emit::Ll), ("ir", Emit::Ir), ("asm", Emit::Asm), ("hex", Emit::Hex)] {
        let c = parse_args(&args(&["a.c", "--device", "p16f877a", "--emit", s])).unwrap();
        assert_eq!(c.emit, want);
    }
}

#[test]
fn rejects_a_missing_device() {
    let e = parse_args(&args(&["a.c"])).unwrap_err();
    assert!(e.contains("--device"), "{e}");
}

#[test]
fn rejects_no_inputs() {
    let e = parse_args(&args(&["--device", "p16f877a"])).unwrap_err();
    assert!(e.contains("no input files"), "{e}");
}

#[test]
fn rejects_an_unknown_flag() {
    let e = parse_args(&args(&["a.c", "--device", "p16f877a", "--wat"])).unwrap_err();
    assert!(e.contains("--wat"), "{e}");
}

#[test]
fn rejects_an_unknown_emit_stage() {
    let e = parse_args(&args(&["a.c", "--device", "p16f877a", "--emit", "bytecode"])).unwrap_err();
    assert!(e.contains("bytecode"), "{e}");
}

#[test]
fn rejects_a_flag_missing_its_value() {
    let e = parse_args(&args(&["a.c", "--device"])).unwrap_err();
    assert!(e.contains("--device"), "{e}");
}
```

Note this test file uses `driver::cli`, so the crate needs a library target alongside its binary.

- [x] **Step 2: Give the driver crate a library target**

Add to `crates/driver/Cargo.toml`, after the `[[bin]]` block:

```toml
[lib]
name = "driver"
path = "src/lib.rs"
```

Create `crates/driver/src/lib.rs`:

```rust
//! Library half of the driver, so the argument parser and the clang/llvm-link
//! discovery logic are unit-testable without spawning the binary.

pub mod clang_discovery;
pub mod cli;
```

In `crates/driver/src/main.rs`, replace `mod clang_discovery;` with `use driver::clang_discovery;` and add `use driver::cli;`.

- [x] **Step 3: Run the tests to verify they fail**

Run: `make test CRATE=driver`
Expected: FAIL, `unresolved import driver::cli`.

Actual: FAIL with `E0583: file not found for module 'cli'`, same root cause, module did not exist yet.

- [x] **Step 4: Implement the parser**

Create `crates/driver/src/cli.rs`:

```rust
//! Hand-rolled argument parsing. The workspace has no external crates and
//! keeps it that way, so there is no `clap` here.

/// Which stage's text artifact to write instead of HEX. The pipeline's stage
/// boundaries are diffable text by design; this exposes them to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emit {
    Ll,
    Ir,
    Asm,
    Hex,
}

#[derive(Debug, Clone)]
pub struct Cli {
    pub inputs: Vec<String>,
    pub output: String,
    pub includes: Vec<String>,
    pub defines: Vec<String>,
    pub device: String,
    pub emit: Emit,
    pub save_temps: Option<String>,
    pub verbose: bool,
}

pub const USAGE: &str = "\
usage: epic-cc [options] <input.c>...

  -o <file>            output file (default: a.hex)
  -I <dir>             include path, repeatable, forwarded to clang
  -D <name[=value]>    define, repeatable, forwarded to clang
  --device <name>      p16f877a | p18f4550 (required)
  --emit <stage>       ll | ir | asm | hex (default: hex)
  --save-temps <dir>   write every stage artifact into <dir>
  -v                   echo the clang and llvm-link commands
";

/// Parse an argument list that does NOT include `argv[0]`.
pub fn parse_args(argv: &[String]) -> Result<Cli, String> {
    let mut inputs = Vec::new();
    let mut output = None;
    let mut includes = Vec::new();
    let mut defines = Vec::new();
    let mut device = None;
    let mut emit = Emit::Hex;
    let mut save_temps = None;
    let mut verbose = false;

    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        // Short flags take their value attached (`-Iinc`) or separate (`-I inc`).
        let short = |name: &str, i: &mut usize| -> Result<Option<String>, String> {
            if let Some(rest) = a.strip_prefix(name) {
                if !rest.is_empty() {
                    return Ok(Some(rest.to_string()));
                }
                *i += 1;
                return argv
                    .get(*i)
                    .cloned()
                    .ok_or_else(|| format!("epic-cc: {name} needs a value"))
                    .map(Some);
            }
            Ok(None)
        };

        if let Some(v) = short("-I", &mut i)? {
            includes.push(v);
        } else if let Some(v) = short("-D", &mut i)? {
            defines.push(v);
        } else if let Some(v) = short("-o", &mut i)? {
            output = Some(v);
        } else if a == "--device" {
            i += 1;
            device = Some(
                argv.get(i).cloned().ok_or("epic-cc: --device needs a value")?,
            );
        } else if a == "--emit" {
            i += 1;
            let v = argv.get(i).cloned().ok_or("epic-cc: --emit needs a value")?;
            emit = match v.as_str() {
                "ll" => Emit::Ll,
                "ir" => Emit::Ir,
                "asm" => Emit::Asm,
                "hex" => Emit::Hex,
                other => return Err(format!("epic-cc: unknown --emit stage {other}")),
            };
        } else if a == "--save-temps" {
            i += 1;
            save_temps = Some(
                argv.get(i).cloned().ok_or("epic-cc: --save-temps needs a value")?,
            );
        } else if a == "-v" {
            verbose = true;
        } else if a.starts_with('-') {
            return Err(format!("epic-cc: unknown option {a}\n\n{USAGE}"));
        } else {
            inputs.push(a.to_string());
        }
        i += 1;
    }

    if inputs.is_empty() {
        return Err(format!("epic-cc: no input files\n\n{USAGE}"));
    }
    let device = device.ok_or_else(|| format!("epic-cc: --device is required\n\n{USAGE}"))?;

    Ok(Cli {
        inputs,
        output: output.unwrap_or_else(|| "a.hex".to_string()),
        includes,
        defines,
        device,
        emit,
        save_temps,
        verbose,
    })
}
```

The closure borrows `argv` and mutates `i`, which the borrow checker will reject as written. If it does, inline the three `short(...)` calls as explicit `if`/`else if` blocks with the same body rather than fighting it; correctness first, brevity second.

Actual: implemented directly with the inlined `if`/`else if` form, skipping the closure attempt.

- [x] **Step 5: Run the tests to verify they pass**

Run: `make test CRATE=driver`
Expected: PASS for the nine `cli.rs` tests. The existing e2e tests still pass, because `main.rs` is not wired to `cli` yet.

Actual: PASS, 9/9 cli.rs, all pre-existing driver e2e tests unaffected.

- [x] **Step 6: Commit** — `cf6be7c`

---

### Task 4: locate `llvm-link` — DONE (acb42c9)

`resolve_clang` at `crates/driver/src/clang_discovery.rs:16` returns `(clang_path, resource_dir)` from `PIC8_CLANG_UNWRAPPED`/`PIC8_CLANG_RESOURCE_DIR`, then from a bundled `clang` beside the executable. `llvm-link` ships in the same directory in both cases (confirmed 2026-08-20: `/opt/clang/bin/llvm-link`, 6 MB).

**Files:**
- Modify: `crates/driver/src/clang_discovery.rs`
- Modify: `crates/driver/tests/` (new test file `llvm_link_discovery.rs`)

**Interfaces:**
- Consumes: `resolve_clang`'s returned clang path.
- Produces: `pub fn resolve_llvm_link(clang: &Path) -> Result<PathBuf, String>`. Task 5 calls it with the path `resolve_clang` returned.

- [x] **Step 1: Write the failing tests**

Create `crates/driver/tests/llvm_link_discovery.rs`:

```rust
use driver::clang_discovery::resolve_llvm_link;
use std::path::Path;

#[test]
fn finds_llvm_link_beside_clang() {
    let dir = std::env::temp_dir().join("epiccc_llvmlink_ok");
    std::fs::create_dir_all(&dir).unwrap();
    let link = dir.join("llvm-link");
    std::fs::write(&link, b"").unwrap();
    let found = resolve_llvm_link(&dir.join("clang")).unwrap();
    assert_eq!(found, link);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn reports_a_clean_error_when_missing() {
    let dir = std::env::temp_dir().join("epiccc_llvmlink_missing");
    std::fs::create_dir_all(&dir).unwrap();
    let e = resolve_llvm_link(&dir.join("clang")).unwrap_err();
    assert!(e.contains("llvm-link"), "{e}");
    std::fs::remove_dir_all(&dir).ok();
}
```

- [x] **Step 2: Run to verify failure**

Run: `make test CRATE=driver`
Expected: FAIL, `cannot find function 'resolve_llvm_link'`.

Actual: FAIL with `E0432: no 'resolve_llvm_link' in 'clang_discovery'`, same root cause.

- [x] **Step 3: Implement**

Append to `crates/driver/src/clang_discovery.rs`:

```rust
/// Find `llvm-link` beside the clang that `resolve_clang` returned. Both the
/// dev image and the release bundle ship them in the same directory, so the
/// clang path is the only input needed.
pub fn resolve_llvm_link(clang: &Path) -> Result<PathBuf, String> {
    let dir = clang
        .parent()
        .ok_or_else(|| format!("clang path has no parent directory: {}", clang.display()))?;
    for name in ["llvm-link", "llvm-link.exe"] {
        let p = dir.join(name);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(format!(
        "llvm-link not found next to clang in {}. It ships with the toolchain bundle; \
         set PIC8_CLANG_UNWRAPPED to a clang whose directory also contains llvm-link.",
        dir.display()
    ))
}
```

- [x] **Step 4: Run to verify passing**

Run: `make test CRATE=driver`
Expected: PASS.

Actual: PASS, 2/2 new, full driver crate suite unaffected.

- [x] **Step 5: Commit** — `acb42c9`

---

### Task 5: wire the driver and migrate every call site — DONE (3dff778)

This is the task that flips the CLI. It must land as one commit, because the moment `main.rs` stops accepting a positional output path, roughly 25 call sites break.

**Files:**
- Modify: `crates/driver/src/main.rs`
- Modify: every `crates/driver/tests/*_e2e.rs` that spawns the binary
- Modify: `crates/fuzz/src/lib.rs` (the `driver_binary()` invocation near `:3032`)
- Modify: `Makefile:51-52` (`compile` target)
- Modify: `README.md:23` (the worked example)
- Modify: `AGENTS.md` if it shows the old invocation

Actual: `crates/driver/tests/e2e.rs` (bare filename, does not match the `*_e2e.rs` glob used to find and batch-migrate the other 21) was missed by the batch pass and fixed as its own edit. `AGENTS.md` had no invocation to migrate, only prose describing the mechanism (`:222`), left untouched.

**Interfaces:**
- Consumes: `cli::parse_args`, `clang_discovery::{resolve_clang, resolve_llvm_link}`, `irparse::sanitize_symbols`, `wholeprog::merge`.
- Produces: the `epic-cc` binary's new command line.

- [x] **Step 1: Find every call site**

```bash
grep -rn "CARGO_BIN_EXE_epic-cc" --include=*.rs crates/ | wc -l
grep -rn "run -q -p driver\|run -p driver" Makefile README.md AGENTS.md
```

Write the list down. Every one of them is migrated in this task.

- [x] **Step 2: Rewrite `main.rs`**

Replace the body of `fn main` in `crates/driver/src/main.rs`. The stages from `irparse::parse_ll` onward (today's lines 59 to 103) are copied across **unchanged**; only the front half changes.

```rust
fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cli = match cli::parse_args(&argv) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    let device = match cli.device.as_str() {
        "p16f877a" => &device::PIC16F877A,
        "p18f4550" => &device::PIC18F4550,
        other => {
            eprintln!("epic-cc: unknown device {other} (expected p16f877a or p18f4550)");
            std::process::exit(2);
        }
    };

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let (clang, resdir) = match resolve_clang(&std::env::vars().collect(), &exe_dir) {
        Ok(pair) => pair,
        Err(msg) => {
            eprintln!("epic-cc: {msg}");
            std::process::exit(1);
        }
    };
    let llvm_link = match resolve_llvm_link(&clang) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("epic-cc: {msg}");
            std::process::exit(1);
        }
    };

    // Temp directory for the per-unit .ll files and the merged one. With
    // --save-temps these become durable artifacts the user can diff.
    let tmp = match &cli.save_temps {
        Some(d) => std::path::PathBuf::from(d),
        None => std::env::temp_dir().join(format!("epic-cc-{}", std::process::id())),
    };
    std::fs::create_dir_all(&tmp).expect("create temp dir");

    // 1. clang: one invocation per translation unit.
    let mut units = Vec::new();
    for (n, input) in cli.inputs.iter().enumerate() {
        let ll_path = tmp.join(format!("{n:03}.ll"));
        let mut cmd = Command::new(&clang);
        cmd.args([
            "-target", "msp430", "-O1", "-S", "-emit-llvm",
            "-ffreestanding", "-nostdinc",
            "-resource-dir", resdir.to_str().unwrap(),
        ]);
        for inc in &cli.includes {
            cmd.args(["-I", inc]);
        }
        for def in &cli.defines {
            cmd.args(["-D", def]);
        }
        cmd.args(["-o", ll_path.to_str().unwrap(), input]);
        if cli.verbose {
            eprintln!("epic-cc: {cmd:?}");
        }
        let out = cmd.output().expect("run clang");
        if !out.status.success() {
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            std::process::exit(1);
        }
        units.push(ll_path);
    }

    // 2. llvm-link: N .ll -> one .ll. Merge order is command-line order, so
    // the renaming of colliding internal symbols is deterministic. Running it
    // for a single unit too keeps one code path; it only rewrites the module
    // header and metadata ordering, which irparse already ignores.
    let merged_path = tmp.join("merged.ll");
    let mut cmd = Command::new(&llvm_link);
    cmd.arg("-S");
    for u in &units {
        cmd.arg(u);
    }
    cmd.args(["-o", merged_path.to_str().unwrap()]);
    if cli.verbose {
        eprintln!("epic-cc: {cmd:?}");
    }
    let out = cmd.output().expect("run llvm-link");
    if !out.status.success() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        std::process::exit(1);
    }

    let ll_text = irparse::sanitize_symbols(
        &std::fs::read_to_string(&merged_path).expect("read merged .ll"),
    );
    if cli.emit == cli::Emit::Ll {
        std::fs::write(&cli.output, &ll_text).expect("write .ll");
        return;
    }

    // 3 onward: unchanged pipeline.
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    if cli.emit == cli::Emit::Ir {
        std::fs::write(&cli.output, ir::serialize(&m)).expect("write ir");
        return;
    }
    m = legalize::legalize(m);
    // ... today's lines 61 to 102, verbatim, with `hex_out` replaced by
    // `cli.output` and an `Emit::Asm` early return after the backend produces
    // `asm` but before `asm::assemble_file_to_hex`.
    std::fs::write(&cli.output, hex).expect("write hex");
}
```

**Do not paraphrase the pipeline half.** Open the file, keep lines 61 to 102 as they are, and change only `hex_out` to `&cli.output`.

Actual: the pipeline body was kept verbatim, but `--emit asm` (a real requirement, not a nicety, per the Interfaces block above) needs an interception point between "final asm text" and "assemble to hex" that the original single `match device.core { ... asm::assemble_file_to_hex(...) }` did not have. Split into two matches: the first produces the final `asm: String` (banking + peephole + page-fit for PIC14, passthrough for PIC18), the second calls `asm::assemble_file_to_hex`. The `Emit::Asm` early return sits between them. No stage crate's behavior changed, only where the driver reads the intermediate value.

- [x] **Step 3: Migrate the call sites**

For each `crates/driver/tests/*_e2e.rs`, the change is mechanical:

```rust
// before
.args(["tests/fixtures/add.c", "tests/fixtures/add.hex"])
// after
.args(["tests/fixtures/add.c", "-o", "tests/fixtures/add.hex", "--device", "p16f877a"])
```

`crates/isel-pic18/tests/e2e.rs` calls the stage crates directly rather than the binary, so it needs no change beyond whatever Task 2 required.

`Makefile:52` becomes:

```make
	@$(DOCKER_RUN) bash -c 'cargo run -q -p driver -- $(FILE) -o /tmp/out.hex --device p16f877a && cat /tmp/out.hex'
```

`README.md:23` becomes:

```console
$ cargo run -p driver -- add.c -o add.hex --device p16f877a && cat add.hex
```

- [x] **Step 4: Run the full suite**

Run: `make test`
Expected: PASS, with every committed `.hex` fixture byte-identical to what is in git. Confirm that explicitly:

```bash
git diff --stat crates/driver/tests/fixtures/
```

Expected: empty. A changed `.hex` here means `llvm-link` or the sanitizer perturbed codegen, and that is a stop-and-investigate, not a "regenerate the golden file".

Actual: `make test` PASS, all 16 crates, exit 0. `git diff --stat crates/driver/tests/fixtures/` returned empty: every golden HEX byte-identical.

- [x] **Step 5: Commit** — `3dff778`

---

### Task 6: multi-TU acceptance test

The end-to-end proof: three translation units with colliding statics, a global defined in one and used in another, and a cross-unit call, compiled by the real binary and run in the simulator.

**Files:**
- Create: `crates/driver/tests/fixtures/multi_tu_main.c`
- Create: `crates/driver/tests/fixtures/multi_tu_a.c`
- Create: `crates/driver/tests/fixtures/multi_tu_b.c`
- Create: `crates/driver/tests/multi_tu_e2e.rs`

**Interfaces:**
- Consumes: the `epic-cc` binary's new CLI from Task 5.
- Produces: nothing other tasks depend on.

- [ ] **Step 1: Write the fixtures**

`multi_tu_main.c`:

```c
// Multi-TU acceptance: `total` is defined here and written by both units;
// `bump` and `scratch` exist in BOTH a.c and b.c as statics, so llvm-link
// must rename one of each pair. Result is computed by hand below.
unsigned char total;
extern unsigned char from_a(unsigned char);
extern unsigned char from_b(unsigned char);

void main(void) {
    total = from_a(3) + from_b(4);   // 4 + 6 = 10 (0x0A)
}
```

`multi_tu_a.c`:

```c
static volatile unsigned char scratch;
__attribute__((noinline)) static unsigned char bump(unsigned char v) { scratch = v; return scratch + 1; }
unsigned char from_a(unsigned char v) { return bump(v); }   // 3 -> 4
```

`multi_tu_b.c`:

```c
static volatile unsigned char scratch;
__attribute__((noinline)) static unsigned char bump(unsigned char v) { scratch = v; return scratch + 2; }
unsigned char from_b(unsigned char v) { return bump(v); }   // 4 -> 6
```

The `noinline` and `volatile` are load-bearing: without them clang `-O1` inlines both helpers away and the collision this test exists to exercise never reaches `llvm-link`. Verified 2026-08-20.

- [ ] **Step 2: Write the failing test**

Create `crates/driver/tests/multi_tu_e2e.rs`:

```rust
use std::process::Command;

#[test]
fn compiles_three_translation_units_end_to_end() {
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/multi_tu_main.c",
            "tests/fixtures/multi_tu_a.c",
            "tests/fixtures/multi_tu_b.c",
            "-o",
            "tests/fixtures/multi_tu.hex",
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/multi_tu.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(2000);
    assert!(p.halted());
    // `total` is the only non-static global; find its address the way the
    // other e2e tests do. If they hard-code it from the alloc layout, do the
    // same here after reading the layout with `--emit ir`.
    // from_a(3) = 3 + 1 = 4, from_b(4) = 4 + 2 = 6, total = 10.
}
```

**Before finishing this test, read `crates/driver/tests/e2e.rs` and `crates/driver/tests/banked_e2e.rs`** to see how they locate a global's RAM address (`e2e.rs` hard-codes `0x20`/`0x21` from the layout). Follow whichever convention they use, and assert `total == 0x0A`.

- [ ] **Step 3: Run to verify it fails**

Run: `make test CRATE=driver`
Expected: FAIL, because the fixtures and the assertion are not complete until Step 2's note is resolved.

- [ ] **Step 4: Complete the assertion and run**

Run: `make test CRATE=driver`
Expected: PASS, `total == 0x0A`.

- [ ] **Step 5: Verify the collision actually happened**

This test is worthless if `llvm-link` never had to rename anything. Prove it:

```bash
make exec CMD='cargo run -q -p driver -- crates/driver/tests/fixtures/multi_tu_main.c crates/driver/tests/fixtures/multi_tu_a.c crates/driver/tests/fixtures/multi_tu_b.c -o /tmp/x.ll --device p16f877a --emit ll && grep -E "^@|^define" /tmp/x.ll'
```

Expected: two `bump` symbols and two `scratch` symbols, one of each pair carrying a sanitized suffix (`bump_3`, `scratch_4` or similar). If you see only one of each, the fixtures were inlined away and the test is not testing what it claims.

- [ ] **Step 6: Commit**

```bash
git add crates/driver/tests/multi_tu_e2e.rs crates/driver/tests/fixtures/multi_tu_*.c crates/driver/tests/fixtures/multi_tu.hex
git commit -m "test(driver): multi translation unit end to end acceptance"
```

---

## Done when

- `make test` passes with every pre-existing `.hex` fixture byte-identical.
- `epic-cc a.c b.c c.c -o out.hex --device p18f4550` compiles a three-unit program.
- `--emit ll|ir|asm|hex` each write the corresponding stage artifact.
- An undefined symbol produces `wholeprog: undefined symbols: <name>`, not an assembler panic.
- `make pre-pr-check` is clean, including the plan-file deletion rule: this file is `git rm`ed in the final commit, and anything load-bearing that this plan discovered is folded back into `docs/31-ecosystem-integration-design.md` D-7 first.

## Known deviation from the spec, to fold back into D-7

D-7 as committed says symbols are sanitized "once, in `wholeprog`". Planning found that a name reaches the IR through `Func.name`, `Global.name`, `Call.func`, `Val::Global`, `GepBase::Global`, `Load.ptr` and `Store.ptr`, so an IR-level walk would have to stay exhaustive across all 20 `Inst` variants indefinitely. Task 1 does it instead as a text transform on the merged `.ll` in `irparse::sanitize_symbols`, which covers every one of those sites in one pass and keeps the stage boundary textual. Update D-7's "Symbols are sanitized once" paragraph accordingly before this branch merges.
