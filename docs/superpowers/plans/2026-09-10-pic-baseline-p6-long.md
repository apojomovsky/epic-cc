# PIC baseline P6: 32-bit long + mul/div (issue #328)

Ephemeral implementation plan. Design of record: `docs/37` D-6 + §3
P6 row (approved 2026-09-09). Delete in the final commit.

## Split (from PIC14 `isel`, via PIC14E's verbatim carryover)

PIC14 inlines i32 add/sub/and/or/xor, const-count shifts, compares,
casts; legalize rewrites mul/div/rem + reg-count shifts to routine
calls whose bodies each backend emits per-copy. Baseline replicates
minus ADDLW/SUBLW/RETURN/PCLATH/IRP-FSR (scouts LongOrigin/Carryover).

## Baseline adaptations (all mechanical)

- `ADDLW k` (add32-const) becomes the D-6 `emit_add_w_const` idiom;
  `SUBLW` paths become `MOVWF tmp / MOVLW k / SUBWF tmp,W`.
- Every direct access gets D-2 `BCF`/`BSF FSR,5`; `fop` masking stays.
- Routine `RETURN` becomes `RETLW 0` after `store_retval`.
- No PCLATH scaffolding anywhere; routines are plain functions under
  the P4 page audit (entries in the page-0 low half).
- Routine frames may span banks: `routine_base`'s single-bank snap
  assumes 128-byte banks, but the biggest integer frame is 20 bytes
  against 16-byte banks. Spanning is sound here (unconditional
  reassertion), so baseline skips the snap. Core-gated, documented.

## Frames (legalize scr contract, params + scr)

mul_u8 8, mul_u16 18, mul_u32 19, udiv/urem_u32 18, sdiv/srem_i32 20
(biggest integer), shifts 5-10. Float reference for Q4: add/sub/mul
14 scr (22 frame), div 12 (20 frame), cmp 6, cvt 8.

## Open question 4 (answered in this phase)

Float working set vs 41-byte GPR with P6 landed: biggest float frame
22 bytes exceeds any 16-byte bank (spanning mandatory, breaking the
shared single-bank assumption), and 22 + user peak + 7 fixed leaves
single digits for any real program. Determination goes in the PR
description; P7 acts on it.

## Fixtures (per-backend copies, like PIC14E)

## Paging (required: routine bodies blow past 256 words)

D-5 covers this ("whichever page they're linked into", PA0 selects):
every CALL site emits a uniform set/restore pair (PA0=callee page
before, PA0=caller page after, always 3 words) so sizes stay
page-independent. Layout packs fragments sequentially: each function
wholly in one page with its entry in a low half (cursor jumps pages
with `org`), tables last under the low-half rule. Fixup bits patch in
place after layout (BCF/BSF swaps, no size change); the P4 re-emit
pass goes away. `verify_page_fit` gains the GOTO rule (target page
must equal PA0 at the GOTO). Routines are leaves (no CALLs inside),
so no nested PA0 state.

## Fixture split (routines are big)

One program using every i32 routine exceeds flash (~920 routine
words). Split by group so each links under 1024 words: an
i32 add/sub/mul fixture (mul_u32 only), an i32 div fixture
(udiv/urem/sdiv/srem_u32), and the i8/i16 muldiv fixture. Each
gpasm- and sim-checked.

  - `long_mul.c`: i32 add/sub/mul, shifts, cmps (mul_u32 only).
  - `long_div.c`: i32 udiv/urem/sdiv/srem.
  - `muldiv.c`: i8/i16 mul/div/rem + shifts (adapted from the sibling).
  - Each gpasm HEX-identity (`-p p12f509`) plus sim; every program must
    satisfy the audit (entries in low halves, one page per function,
    total under 1024 words).

## Non-goals

Hardware multiply (none on baseline; AN526 software loops), ISR
routine copies (no interrupts), float implementation (Q4 answers
whether it fits; P7 acts).
