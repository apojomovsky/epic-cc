// Milestone-12 "long" acceptance: a straight-line program exercising the
// whole i32 surface — add via a noinline call, mul, udiv, urem, sdiv, srem,
// const-count shl/lshr/ashr (inline), icmp ult/ugt/ule, trunc/zext/sext
// (i32->i8/i16 and i8/i16->i32), and a struct with a long member via
// byval/sret ({i8,i32}, 6-byte layout) — compiles through the whole driver
// pipeline and runs correctly in the simulator.
//
// Shape notes (clang -O1 folds aggressively):
//   - `in`, `sin`, `out`, `sp`, `g8`, `g16` are volatile: `out` is written
//     after every step so no step is DCE'd.
//   - The udiv divisor 0x10F and the urem divisor 0x10E are non-power-of-two
//     (a pow2 divisor strength-reduces to a shift / and) and distinct (a
//     shared divisor would let clang reuse the div-rem pair, dropping the
//     urem routine call).
//   - `sin` is a volatile signed long (-19 = 0xFFFFFFED): the sdiv / srem /
//     ashr and the i8/i16 sexts need a value whose sign bits are set.
//   - Genuine trunc/zext/sext IR ops: `(unsigned char)x` on an i32 is a
//     `trunc`, but clang folds `(unsigned short)x` and `(short)/(signed
//     char)` re-widening of a *known* value to `and`/`shl+ashr` idioms. The
//     volatile g8/g16 round-trips force i8/i16 SSA values, so `(unsigned
//     long)g8` / `(long)(signed char)g8` emit real `zext`/`sext` ops (the
//     i16 zext/sext come from the g16 round-trip of `sin`'s low half,
//     whose bit 15 is set — the m12 sign-fill path).
//   - main's live locals are exactly 9 x i32 (36 bytes): the runtime
//     routines' frames (base = frame_end(main), biggest __sdiv_i32 at 20
//     bytes) must stay inside bank 0 (end <= 0x6F) — the loud isel bank-0
//     assert fires otherwise. Every non-routine op (the adds/shifts, the
//     icmps, the casts, the struct byval/sret) lives in the noinline helper
//     `misc`, whose frame has no bank constraint (it calls no runtime
//     routine). The udiv/mul chain and the urem are split into two locals
//     (`m`, `u`) so the merge-add happens in misc, keeping main at 9 defs.
//   - No i32 const tables and no const-const i32 ops (deferred — both still
//     panic loudly).
//
// Expected: in = 0x12345678, sin = -19 -> out = 0x1634943A (traced in
// crates/driver/tests/long_e2e.rs against the exact emitted IR).
struct P { unsigned char a; unsigned long b; };   // {i8,i32}: a@0, b@2, size 6

volatile unsigned long  out;
volatile unsigned long  in;
volatile long           sin;
volatile struct P       sp;
volatile unsigned char  g8;
volatile unsigned short g16;

__attribute__((noinline)) unsigned long addm(unsigned long a, unsigned long b) {
    return a + b;
}
__attribute__((noinline)) unsigned long getb(struct P p) {   // byval 6 bytes
    return p.b;                                              // offset 2, 4 bytes
}
__attribute__((noinline)) struct P mkp(unsigned long v) {    // sret
    struct P r;
    r.a = 0xAB;
    r.b = v;
    return r;
}
__attribute__((noinline)) unsigned long misc(unsigned long a, unsigned long m,
                                             unsigned long u, long s) {
    unsigned long r = (m + u) << 3 | (m + u) >> 1;   // add, shl, lshr, or i32
    r += (unsigned long)(s >> 4);                    // ashr i32 const (sign-fill)
    r += (a < 0x20000000) ? 1 : 0;                   // icmp ult
    r += (a > 0x1000) ? 2 : 0;                       // icmp ugt
    r += (a <= 0x12345678) ? 4 : 0;                  // icmp ule (canonicalized)
    unsigned char b0 = (unsigned char)a;             // trunc i32->i8
    g8 = b0;
    r += (unsigned long)g8;                          // zext i8->i32
    r += (unsigned long)(long)(signed char)g8;       // sext i8->i32
    g16 = (unsigned short)s;                         // trunc i32->i16 (low half)
    unsigned short h = g16;
    r += (unsigned long)h;                           // zext i16->i32
    r += (unsigned long)(long)(short)h;              // sext i16->i32
    sp = mkp(a);                                     // sret call -> volatile global copy
    r += getb(sp);                                   // byval call from the global
    return r;
}

void main(void) {
    unsigned long a = in;                            // 0x12345678
    out = addm(a, 5);                                // i32 add via noinline call
    unsigned long m = (a / 0x10F) * 7;               // udiv i32, mul i32
    unsigned long u = a % 0x10E;                     // urem i32
    long s = sin;                                    // -19
    out = (unsigned long)(s / -3);                   // sdiv i32
    out = (unsigned long)(s % 3);                    // srem i32
    out = misc(a, m, u, s);
}
