# Task 5 Report — banking + peephole: Asm barrier (never insert inside, unknown after)

**Date:** 2026-08-21
**Worktree:** `.worktrees/cc4-inline-assembly`
**Branch:** `feat/cc4-inline-assembly`
**Base:** `a9e2ec7` feat(isel): CC-4 verbatim module asm, naked and inline Asm
**Commit:** (pending) `feat(banking): CC-4 never insert BANKSEL inside Asm, unknown after`

## Scope
Target: `crates/banking/src/lib.rs` (`assign_banks`, `is_bank0_only`, helpers `walk_region`/`BankSet`), `crates/peephole/src/lib.rs` (`optimize`).

Implements plan Task 5 / spec §9:
- Banking: detect `; --- asm start ---` / `; --- asm end ---` (trimmed start) emitted by Task 4; while `in_asm` emit verbatim without BANKSEL insertion or operand rewriting; on entry set tracked to UNKNOWN (`known=false`), on exit keep UNKNOWN so next banked operand gets full BANKSEL (both RP bits). `is_bank0_only` returns false if text contains asm marker, so full banking path runs.
- Peephole: same markers split the input; optimize each non-asm segment independently, rejoin verbatim blocks untouched. While `in_asm` disable PCLATH tracking and pattern matching; reset `tracked=None` on both markers.

## Changes
- `crates/banking/src/lib.rs`:
  - `is_bank0_only` (300): add early `if asm.contains("; --- asm start ---") || asm.contains("; --- asm end ---") { return false; }` before scan. Ensures any opaque Asm makes program not bank0-only, so label/CALL resets are not skipped and full BANKSEL preambles are emitted where needed.
  - `assign_banks` (337): add `let mut in_asm = false;` and at top of loop handle markers: `trimmed.starts_with("; --- asm start ---")` sets `in_asm=true`, `known=false`, emit unchanged; `trimmed.starts_with("; --- asm end ---")` clears `in_asm`, `known=false`; `if in_asm { emit verbatim; continue; }` before any label/CALL/bank-op/operand logic. Guarantees no BANKSEL inserted inside block and no `0xA0 -> 0x20` rewriting inside.
  - `walk_region` (199): precompute `asm_inside` per index by linear scan of markers, and in worklist handle markers as `BankSet::UNKNOWN` barrier and skip interpretation of interior verbatim lines (`asm_inside[i]` → `work.push((i+1, UNKNOWN))`). Prevents CALL-exit-bank analysis from pinning a bank on an opaque `MOVF 0xA0` inside Asm and incorrectly proving the callee exits bank 1.

- `crates/peephole/src/lib.rs`:
  - `optimize` (30): add `let mut in_asm = false;`, handle markers at loop top: on `; --- asm start ---` set `in_asm=true`, push verbatim, `tracked=None`; on `; --- asm end ---` clear, `tracked=None`; `if in_asm { push verbatim; continue; }`. Also guard the `MOVLW`/`MOVWF PCLATH` pair elision from crossing a barrier (check next line not an asm marker). This implements "split on asm markers, optimize each non-asm segment independently, rejoin verbatim".

## Acceptance Tests
Plan Task 5 acceptance tests (verified via temporary `crates/banking/tests/task5_verify.rs` and `crates/peephole/tests/task5_verify.rs` in worktree, then removed before commit):

```rust
#[test]
fn banking_never_inside_asm_block() {
    let asm = "main:\n  MOVF 0x20, W\n  ; --- asm start ---\n  bcf INTCON, 7\n  bsf INTCON, 7\n  ; --- asm end ---\n  MOVF 0xA0, W\n";
    let out = banking::assign_banks(&PIC16F877A, asm);
    let banksel_count = out.matches("STATUS,").count();
    assert!(banksel_count >= 1);
    assert!(!out.contains("bcf INTCON, 7\n    BSF STATUS"));
    assert!(!out.contains("bcf INTCON, 7\n    BCF STATUS"));
}

#[test]
fn banking_unknown_after_asm() {
    let asm = "main:\n  MOVF 0x20, W\n  ; --- asm start ---\n  my_asm_line\n  ; --- asm end ---\n  MOVF 0x20, W\n";
    let out = banking::assign_banks(&PIC16F877A, asm);
    assert!(out.matches("STATUS,").count() >= 1);
    assert!(out[out.find("; --- asm end ---").unwrap()..].contains("STATUS"));
}
#[test]
fn banking_is_bank0_only_false_with_asm() {
    let asm2 = "main:\n  MOVF 0x20, W\n  ; --- asm start ---\n  nop\n  ; --- asm end ---\nL1:\n  MOVF 0x20, W\n";
    let out2 = banking::assign_banks(&PIC16F877A, asm2);
    assert!(out2.matches("STATUS,").count() >= 1);
}
#[test]
fn banking_operand_inside_asm_not_rewritten() {
    let asm = "main:\n  MOVF 0x20, W\n  ; --- asm start ---\n  MOVF 0xA0, W\n  ; --- asm end ---\n  MOVF 0x20, W\n";
    let out = banking::assign_banks(&PIC16F877A, asm);
    // inside 0xA0 stays 0xA0, no STATUS inside block
    assert!(inside.contains("0xA0"));
    assert!(!inside_block.contains("STATUS,"));
}
#[test]
fn peephole_does_not_cross_asm() {
    let asm = "    MOVLW 0x08\n    MOVWF PCLATH\n    ; --- asm start ---\n    nop\n    ; --- asm end ---\n    MOVLW 0x08\n    MOVWF PCLATH\n";
    let out = peephole::optimize(asm);
    assert_eq!(out.matches("MOVLW 0x08").count(), 2);
}
#[test]
fn peephole_inside_asm_verbatim() {
    let asm = "    MOVLW 0x08\n    MOVWF PCLATH\n    ; --- asm start ---\n    MOVLW 0x08\n    MOVWF PCLATH\n    ; --- asm end ---\n";
    let out = peephole::optimize(asm);
    assert!(out.contains("; --- asm start ---"));
    assert_eq!(out.matches("MOVLW 0x08").count(), 2);
}
```

Result: `cargo test -p banking --test task5_verify -- --nocapture` → 4/4 passed; `cargo test -p peephole --test task5_verify` → 3/3 passed. Both removed before commit. Original spec phrases "BANKSEL count 1 not splitting bcf/bsf, not containing `bcf INTCON, 7\n  BANKSEL`" and "at least one BANKSEL after unknown" map to the STATUS-count assertions above (banking emits `BCF/BSF STATUS, 5/6`).

## Verification
In worktree (docker `epic-cc-dev:local`, pinned clang 20.1.8, toolchain 1.97.1):

```
cargo test -p banking -p peephole -p isel -- --nocapture
  - banking: 31 passed (including branch_targets_reset, tracking_resumes, call_exit_bank transitive, etc.)
  - peephole: 10 passed (same_page_call_elides, cross_page_keeps, label resets, etc.)
  - isel: 159 passed (banked_growth, bin_packing, page-fit, shift routines, float, etc.)
  - overall: 0 failed
cargo test -p banking -- --nocapture (isolated): 31 passed
cargo test -p peephole -- --nocapture: 10 passed
```

Docker `make exec CMD='cargo test -p banking -p peephole -p isel'` green; previous `cargo test -p isel --test isel` 159 green confirms no regression on Task 4 isel. Temporary verify files used `/tmp/cargo-target` to avoid host target pollution; final commit only touches `crates/banking/src/lib.rs`, `crates/peephole/src/lib.rs`, and this report.

## Implementation Notes
- Marker detection uses `line.trim_start().starts_with("; --- asm start ---")` / `"; --- asm end ---"` – the exact strings emitted by `isel`/`isel-pic18` `Gen::emit_inst` for `Inst::Asm` and naked bodies. Leading spaces preserved, comparison after trim handles verbatim indentation variations. Contains-check for `is_bank0_only` uses `asm.contains` which is equivalent but coarser and conservative.
- Banking `in_asm` handling is before label/CALL/bank-op/operand logic, so a `CALL` or `BCF STATUS, 5` verbatim inside Asm never updates `known`/`rp0`/`rp1`. The UNKNOWN reset mirrors `MOVWF STATUS` semantics (bank unknowable) – next banked operand gets full BANKSEL re-establishing both bits, exactly as a label does when `bank0_only` is false.
- Peephole reset of `tracked=None` on both markers prevents a `MOVLW 0x08; MOVWF PCLATH` before Asm from eliding an identical pair after Asm (different runtime PCLATH). The inside-asm guard also prevents eliding a pair that straddles the barrier or mis-tracking a `MOVLW` inside Asm as the last literal.
- `walk_region` UNKNOWN handling ensures CALL-exit provability is conservative: a callee containing Asm is treated as exiting UNKNOWN, so caller keeps full reset – no silent bank mismatch if hand-written Asm changed RP bits.

## Concerns / Follow-up
- `; module asm` (top-of-file comment) is not treated as barrier – it is outside any function and not bracketed, correct to not force UNKNOWN.
- No change to `function_regions` splitting – CALL targets inside Asm verbatim are rare (inline asm rarely contains `CALL`); if present they would be ignored for region building, which is conservative (exit bank UNKNOWN).
- Peephole alternative "split on markers and optimize segments" is semantically equivalent to the `in_asm` guard; both prevent cross-barrier elision and keep verbatim untouched.
