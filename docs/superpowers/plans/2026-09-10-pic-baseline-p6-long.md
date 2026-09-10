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

- `long.c`: whole-i32 surface adapted to the 509 budget (sized down
  from the 877A original), seed in=0x12345678, sin=-19.
- `muldiv.c`: i8/i16 mul/div/rem + shifts, seed in=301, out=210.
- Both gpasm HEX-identity (`-p p12f509`) plus sim, mirroring P4.
- Total program (routines included) must satisfy the P4 audit:
  entries < 0x100, code < 0x200, total < 0x400.

## Non-goals

Hardware multiply (none on baseline; AN526 software loops), ISR
routine copies (no interrupts), float implementation (Q4 answers
whether it fits; P7 acts).
