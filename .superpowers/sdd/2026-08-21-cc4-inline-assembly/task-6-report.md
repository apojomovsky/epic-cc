# Task 6 Report — driver: EPIC_NAKED and __epic_* intrinsics, .asm rejection

**Date:** 2026-08-21
**Worktree:** `.worktrees/cc4-inline-assembly`
**Branch:** `feat/cc4-inline-assembly`
**Base:** `3195f01` feat(banking): CC-4 never insert BANKSEL inside Asm, unknown after
**Commit:** (pending) `feat(driver): CC-4 EPIC_NAKED and __epic_* intrinsics`

## Scope
Target: `crates/driver/src/epic_cc_h.rs`, `crates/driver/src/main.rs` (CLI handling before clang loop), tests.

Implements plan Task 6 / spec §6:
- Extend `epic-cc.h` with `EPIC_NAKED` and header-only intrinsics `__epic_nop`, `__epic_clrwdt`, `__epic_sleep`, `__epic_di`, `__epic_ei`, each one-liner expanding to single opaque `asm volatile("...")` block so they reuse the `Inst::Asm` path.
- Reject `.asm`/`.s` inputs case-insensitive before clang loop with precise message and exit 2, directing to `EPIC_NAKED`.
- Add `header_has_epic_naked` test.

## Changes
- `crates/driver/src/epic_cc_h.rs` (7-36):
  - After `EPIC_FOSC_HZ` guard, add:
    ```c
    #define EPIC_NAKED __attribute__((naked))
    #define __epic_nop()    asm volatile("nop")
    #define __epic_clrwdt() asm volatile("clrwdt")
    #define __epic_sleep()  asm volatile("sleep")
    #define __epic_di()     asm volatile("bcf INTCON, 7")
    #define __epic_ei()     asm volatile("bsf INTCON, 7")
    ```
    All one-liners, single opaque asm block, header-only. Preserves existing `EPIC_AT`, `EPIC_CONFIG`, `EPIC_FOSC_HZ`, `EPIC_CC_H` guard.
  - Add `#[cfg(test)] mod tests` with `header_has_epic_naked`:
    - Asserts `EPIC_CC_H.contains("EPIC_NAKED")`, each `__epic_*`, `asm volatile("nop")` etc., and `__attribute__((naked))`.
    - Covers single-opaque-block requirement explicitly.

- `crates/driver/src/main.rs` (40-46):
  - After `cli::parse_args` success, before device resolution and before temp header write / clang loop:
    ```rust
    for input in &cli.inputs {
        let lower = input.to_ascii_lowercase();
        if lower.ends_with(".asm") || lower.ends_with(".s") {
            eprintln!("epic-cc: .asm inputs are not supported in this build; use EPIC_NAKED functions");
            std::process::exit(2);
        }
    }
    ```
    Case-insensitive, matches `.asm` and `.s` (hence `.ASM`, `.S`). Placed before `resolve_clang` and before per-unit clang invocations, so no clang spawned.

## Acceptance Tests
Plan Task 6 acceptance (verified in worktree):

```rust
#[test]
fn header_has_epic_naked() {
    assert!(EPIC_CC_H.contains("EPIC_NAKED"));
    assert!(EPIC_CC_H.contains("__epic_nop"));
    assert!(EPIC_CC_H.contains("__epic_clrwdt"));
    assert!(EPIC_CC_H.contains("__epic_sleep"));
    assert!(EPIC_CC_H.contains("__epic_di"));
    assert!(EPIC_CC_H.contains("__epic_ei"));
    assert!(EPIC_CC_H.contains("asm volatile(\"nop\")"));
    assert!(EPIC_CC_H.contains("asm volatile(\"clrwdt\")"));
    assert!(EPIC_CC_H.contains("asm volatile(\"sleep\")"));
    assert!(EPIC_CC_H.contains("asm volatile(\"bcf INTCON, 7\")"));
    assert!(EPIC_CC_H.contains("asm volatile(\"bsf INTCON, 7\")"));
    assert!(EPIC_CC_H.contains("__attribute__((naked))"));
}
```

Result: `cargo test -p driver --lib epic_cc_h::tests::header_has_epic_naked -- --nocapture` → ok (5/5 lib tests pass).

CLI rejection verified manually and via integration-style check:

```
cargo run -q -p driver -- --help 2> /tmp/err; echo STATUS:$?
  -> STATUS:2, prints "epic-cc: unknown option --help" + usage (still works)

cargo run -q -p driver -- /tmp/foo.asm 2> /tmp/err; echo STATUS:$?
  -> STATUS:2, "epic-cc: .asm inputs are not supported in this build; use EPIC_NAKED functions"

cargo run -- /tmp/foo.ASM  /tmp/foo.s  /tmp/foo.S -> same STATUS:2 and message
```

Normal `.c` inputs remain unaffected (e2e `add.c`, `banked.c`, etc. still compile).

## Verification
In worktree (docker `epic-cc-dev:local`, pinned clang 20.1.8, toolchain 1.97.1):

```
make -C .worktrees/cc4-inline-assembly exec CMD='cargo test -p driver --lib -- --nocapture'
  - 5 passed (4 clang_discovery + header_has_epic_naked)

make -C .worktrees/cc4-inline-assembly exec CMD='cargo test -p driver -- --nocapture'
  - lib: 5 passed
  - cli integration: 9 passed
  - e2e fixtures: const_table, const_struct, array, float, banking, interrupts, etc.: all green
  - overall: 0 failed (full crate green, same as Task5 baseline +1)

make exec CMD='cargo run -q -p driver -- --help' -> usage printed, exit 2 (unknown option path unchanged)
make exec CMD='cargo run -q -p driver -- /tmp/foo.asm; echo STATUS:$?' -> STATUS:2 with expected message
make exec CMD='cargo run -q -p driver -- /tmp/foo.ASM / -s variants' -> case-insensitive rejection verified
cargo test -p driver (isolated, after cargo clean -p driver) -> header test discovered via --list
```

Prior 4 tasks' crates unchanged; `cargo test -p banking -p peephole -p isel` remain green (31 + 10 + 159).

## Implementation Notes
- Header choice: one-liner macros avoid multi-line backslash continuation beyond `EPIC_CONFIG`; each intrinsic is `asm volatile("...")` exactly, so `irparse` lifts it as single `Inst::Asm { template, clobbers_memory:false }`. No `"memory"` clobber – they are single-instruction barriers but ordering preserved by `asm volatile` alone (probed volatile ordering).
- `EPIC_NAKED` maps directly to `__attribute__((naked))`; `irparse` already detects `naked` in the attribute list (`#0 = { naked noinline }`) – no driver-side device define needed.
- `.asm`/`.s` rejection in `main.rs` rather than `cli::parse_args` keeps `cli.rs` pure parser (no process exit) and places the check exactly "before clang loop" as spec requires. The message is byte-identical to spec; exit 2 matches `parse_args` error exit code, so CI that checks for exit 2 on bad inputs remains consistent.
- Case-insensitive via `to_ascii_lowercase()` covers `.ASM`, `.S`, `.aSm`, etc.; `ends_with(".s")` also matches `.s` but not `.so` or `.s123` – correct per spec (only `.s` extension).
- No change to `cli.rs`/`lib.rs` signatures; header is written to temp `include/epic-cc.h` as before, now with new defines – existing fixtures that include `<epic-cc.h>` continue to compile without modification.
