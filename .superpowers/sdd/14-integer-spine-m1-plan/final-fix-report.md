# Final-Review Fix Report — integer-spine milestone 1

Branch: `feat/integer-spine`

Two final-review findings fixed in one wave, plus regression tests.

## Finding 1 (Critical) — isel materialized constants as file-register ADDRESSES

`crates/isel/src/lib.rs`

Root cause: `val_addr(Val::Const(k))` returned the literal `k` as a byte
address, so both the Store arm and the Bin (add) arm emitted a file-register
read (`MOVF 0x05, W` / `ADDWF 0x05, W`) for a constant operand — a read of
SFR `0x05` (PORTA), not the constant 5.

Fixes:
- **Store arm**: when `s.val` is `Val::Const(k)`, emit `MOVLW 0x..` then
  `MOVWF <dst>`; the `MOVF` path is kept only for Reg/Global sources.
- **Bin add arm**: normalize the commutative add — a `Val::Const` LHS is
  swapped to the RHS so the existing add-const arm (`MOVF` of the register +
  `ADDLW`) handles it; if both operands are const, panic loudly
  (`isel: constant folding not implemented`). A const operand is never read as
  a file register.
- i8 width asserts retained (plus a const byte-range assert `0..=255`).

## Finding 2 (Important) — asm masked f to 7 bits without a range check

`crates/asm/src/lib.rs`

Root cause: the `f` closure applied `& 0x7F` silently, so `MOVWF 0x80`
assembled to `MOVWF 0x00` instead of panicking.

Fix: `assert!(v <= 0x7F, "asm: file register 0x{v:02X} out of range")` is added
inside the `f` closure before masking.

## Regression tests added

- `crates/isel/tests/isel.rs`
  - `store_const_emits_movlw_not_movf`: `store i8 5 @out` → asm contains
    `MOVLW 0x05` and `MOVWF 0x21`, and does NOT contain `MOVF 0x05`.
  - `add_const_lhs_uses_addlw`: `%x = add i8 5, %1` → asm uses the ADDLW path
    (contains `ADDLW 0x05`), and does NOT contain `ADDWF 0x05` / `MOVF 0x05`.
- `crates/asm/tests/assemble.rs`
  - `panics_on_file_register_out_of_range`: `#[should_panic(expected =
    "asm: file register 0x80 out of range")]` for a program with `MOVWF 0x80`.

## Verification

Command: `nix develop --command cargo test --workspace`

Result: all test suites pass, no failures.
- `tests/isel.rs`: 4 passed
- `tests/assemble.rs`: 4 passed
- `tests/gpasm_cross.rs`: 1 passed
- every other crate suite green (alloc, banking, callgraph, driver e2e, ir
  roundtrip, irparse, legalize, peephole, pic14_sim, wholeprog).

## Commits

- `fix(isel): materialize constants instead of file-register reads`
- `fix(asm): assert file register range`
