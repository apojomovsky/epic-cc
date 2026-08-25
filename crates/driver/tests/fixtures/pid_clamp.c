// epic-cc#133 acceptance: the pid clamp pattern (signed min/max on i16,
// then a 16x16 -> 32 signed multiply feeding an i16 truncate).
//
// Shape notes (clang -O1 folds aggressively):
//   - `in_*` are NON-volatile: a volatile operand's compare would stay an
//     icmp/select, but the real pid clamp uses plain locals, which clang
//     folds into `llvm.smax`/`llvm.smin`/`llvm.abs` intrinsic calls, the
//     exact surface this ticket lowers. Volatile inputs would not exercise
//     the intrinsics.
//   - `out` is volatile so the chain survives DCE.
//   - The 16x16 -> 32 signed multiply is written as the abs/sign idiom
//     clang lowers to two `llvm.abs.i16` calls plus a `mul nuw i32` and a
//     sign select; `(long)a * (long)b` would be a `mul nsw i32` instead,
//     which does not produce the abs intrinsics.
//
// Expected (traced in pid_clamp_e2e.rs): in_a = -3000, in_min = -1000,
// in_max = 1000, in_b = 15.
//   clamp(-3000, -1000, 1000) = -1000      (smax/smin i16)
//   mul_s16(-1000, 15): abs both, product |1000*15| = 15000, neg = true
//     -> -15000 (the i32 mul + the sign select)
//   p >> 8 = -15000 >> 8 = -59 (arithmetic), trunc i32 to i16 = -59
//   out = -59 (0xFFC5)
volatile short out;
short in_a, in_min, in_max, in_b;

static long mul_s16(short a, short b)
{
    int neg = ((a < 0) != 0) ^ ((b < 0) != 0);
    unsigned short ua = (unsigned short)a;
    unsigned short ub = (unsigned short)b;
    if (a < 0) { ua = (unsigned short)(0u - (unsigned short)a); }
    if (b < 0) { ub = (unsigned short)(0u - (unsigned short)b); }
    unsigned long ur = (unsigned long)ua * (unsigned long)ub;
    if (neg) {
        ur = (unsigned long)(-(long)ur);
    }
    return (long)ur;
}

int main(void)
{
    short output = in_a;
    if (output < in_min) { output = in_min; }
    if (output > in_max) { output = in_max; }
    long p = mul_s16(output, in_b);
    out = (short)(p >> 8);
    return 0;
}
