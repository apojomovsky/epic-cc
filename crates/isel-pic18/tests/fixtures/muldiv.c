// Milestone-8 mul/div/mod/shift acceptance: a straight-line program that
// exercises the whole new scalar surface  -  mul, udiv, urem, sdiv, srem,
// shl (const), lshr (const), and a variable-count shl  -  on both i16 and
// i8, with a hand-computable `out`.
//
// Notes on why the C is shaped the way it is (clang -O1 folds aggressively):
//   - `out` is volatile and written after every step: a plain local
//     accumulator would let clang DCE the earlier steps (later
//     `out = ...` overwrites without reading), and the volatile reloads
//     pin every op in the IR.
//   - `a % 7` right after `a / 7` becomes `a - (a/7)*7` (div-rem pair
//     reuse), so the urem divisor is 5, distinct from the udiv divisor 7.
//   - `int b = -19` folds every `b / -3` / `b % 3` to constants, so b is
//     made runtime-dependent: `(int)a - 320` == -19 for in == 301.
//   - `(c * 7) / 3` stays i16 (C promotes char to int), so the i8 mul and
//     i8 udiv come from explicit i8 casts (`unsigned char` temporaries),
//     with a volatile `gate` round-trip so clang keeps genuine i8 SSA
//     values instead of widening the whole chain to i16.
//   - the steps are merged just enough (`(out + v) * 5`) to keep main's
//     frame end <= 0x5E: the runtime recipes' slots must fit one GPR bank
//     (issue #6; a straddling routine frame rounds into bank 1 wholesale,
//     and the plan's unmerged shape spills __mul_u16's 14-byte scratch
//     across the bank-0/1 boundary).
//
// Expected: in = 301 -> out = 210 (traced in muldiv_e2e.rs against the
// exact emitted IR; the plan's shape recomputed  -  clang strength-reduced
// `a % 7`, folded the constant `b`, and widened the i8 chain).
volatile unsigned int out;
volatile unsigned int in;
volatile unsigned char gate;

void main(void) {
    unsigned int a = in;                     // 301
    out = a / 7;                             // udiv i16: 43
    out = (out * 3) + (a % 5);               // mul i16 + urem i16: 129 + 1 = 130
    out = out << 2;                          // shl i16 const: 520
    out = (out >> 3) | (a >> 4);             // lshr i16 const: 65 | 18 = 83
    int b = (int)a - 320;                    // -19 (runtime, so sdiv/srem survive)
    out = (unsigned int)(b / -3);            // sdiv i16: 6
    out = (unsigned int)(b % 3) + out;       // srem i16: -1 (0xFFFF) + 6 = 5
    unsigned char c = (unsigned char)a;      // 45
    gate = c;                                // barrier: genuine i8 SSA for c
    c = gate;
    unsigned char w = (unsigned char)(c * 7);   // mul i8: (45*7)&0xFF = 59
    unsigned char v = (unsigned char)(w / 3);   // udiv i8: 59/3 = 19
    out = (out + (unsigned int)v) * 5;       // mul i16: (5 + 19) * 5 = 120
    out = out + ((unsigned int)c << (unsigned char)(a & 3));  // shl var i16: 120 + 45<<1 = 210
}
