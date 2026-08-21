# ADR-015: PIC18 soft-float (port of PIC14 recipes, skip-sensitive frame rule)

**Status:** Accepted 2026-08-20 (implemented in feat/pic18-p7-softfloat)

## Decision

P7 ports PIC14's nine IEEE754 f32 runtime recipes into `isel-pic18`:

* Shared helpers: `emit_f32_extract` (sign/exp/mantissa decode, class dispatch, implicit-bit OR), `emit_f32_assemble`, `emit_f32_round_up` (RNE), `emit_f32_neg_zero`/`inf`/`nan`.
* Bodies: `__add_f32`/`__sub_f32` (shared add body; sub flips `b` sign), `__mul_f32`, `__div_f32`, `__cmp_f32` (tri-state 0/1/2/3), `__uitofp_f32`/`__sitofp_f32`, `__fptoui_f32`/`__fptosi_f32`.
* Substitution: `RLF`→`RLCF f,F,A`, `RRF`→`RRCF f,F,A`, `BTFSC`/`BTFSS` `STATUS,0/2`→`0xFD8,0/2,A`, every file op via `operand()` for `,A`/`,B`.
* Frame rule: every float routine slot (`a`/`b`/`val` + `__scr` + retval) must sit at `≤0x5F` (access-bank GPR, `a=0`, no `MOVLB`). `emit_routine` asserts this at emission (the PIC18 analog of PIC14's `assert_bank0`). `float.c` fits.
* `_isr` variants share the base recipe via `routine_recipe` stripping, reading their own slots.

## Rationale

The float algorithms (decode/encode, implicit bit, RNE guard/sticky/LSB, denormal handling, div's restoring iterations, fcmp tri-state) are machine-verified on PIC14. Porting them line-for-line with the substitution table preserves the verification investment. The frame rule replaces PIC14's single-GPR-bank constraint with the same reason: a `MOVLB` between a skip-test and its target breaks the skip. D-1's "written twice is accepted" covers the duplication.

## Rejected alternatives

* Share the recipes via a trait between `isel` and `isel-pic18` (P6 ADR-014 same reasoning: the instruction sets differ enough that sharing leaks an abstraction; PIC14's `PCLATH`/`BANKSEL` vs PIC18's `TBLRD`/`MOVLB` already forced separate crates).
* Re-derive the algorithms (unnecessary risk; the port is mechanical).

## Revisit if

A part with a different soft-float story appears, or the access-bank frame rule proves too tight for a larger float program (the rule could be relaxed to allow `MOVLB` outside skip windows, or the recipes could be rewritten branch-based like P6's divmod).
