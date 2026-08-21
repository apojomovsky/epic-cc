// Milestone-15 soft-float acceptance: a straight-line program exercising
// the whole float surface — an fdiv call through a noinline helper (half),
// fadd, fmul, the fptosi + sitofp int round trip, the fcmp `olt`
// predicate, the RNE case 1.0f/3.0f (0x3EAAAAAB — the load-bearing
// rounding check), and a struct with a float member via sret (mk) and
// byval (pick) — compiles through the whole driver pipeline and runs
// correctly in the simulator.
//
// Shape notes (clang -O1 folds aggressively):
//   - `in`, `out1..out3` are volatile: every step's result is written to a
//     distinct volatile global, so no step is DCE'd and each can be
//     asserted.
//   - `half` is noinline with a NON-power-of-two divisor: clang folds
//     `a / 2.0f` to `a * 0.5f` (exact reciprocal) in the frontend, so the
//     brief's literal half would not contain an fdiv. `a / 2.5f` survives
//     as a genuine fdiv call (0.4 is inexact, so no fold without
//     fast-math). out1 = 3.0/2.5 = 1.2 = 0x3F99999A (a second RNE case).
//   - The brief's literal `1.0f / 3.0f` also folds to a constant (no
//     __div_f32 call). `1.0f / a` (a = 3.0f, runtime) is the same 1.0/3.0
//     arithmetic with a real fdiv call and the same RNE result
//     0x3EAAAAAB, stored into the struct.
//   - The round trip `(float)(int)((a + 0.25f) * 3.0f)` merges the fadd,
//     the fmul and both conversions into one observed value (out2): the
//     add and mul are exact (3.25, 9.75 — any >= 1 ulp error changes the
//     truncation to a different integer), and the fptosi sees a genuinely
//     fractional value. The exact add/mul VALUES are pinned bit-for-bit by
//     the Task-3 SIM tests; this step pins the end-to-end chain.
//   - `struct_step` is the struct surface: `mk` (sret) builds
//     `{c, f} = {(unsigned char)(a < 0.75f), 1.0f / a}` into a local
//     struct; `pick` (byval) returns `s.c ? 0.0f : s.f` — an i8 icmp +
//     float select with NO float arithmetic, so struct_step's frame sits
//     outside the routine-slot chain. The flag is FALSE for a = 3.0
//     (`fcmp olt` with the brief's operand order), so pick selects s.f:
//     the fcmp result (via c), the fdiv RNE result (via f) and the
//     sret/byval byte copies all flow into out3 — any one broken changes
//     out3 from 0x3EAAAAAB.
//   - Frame budget (bank 0 GPR 0x20-0x6F): globals end at 0x30; main's 11
//     defs (33 bytes) end at 0x51; half's frame (8) ends at 0x59, and
//     __div_f32's 20-byte slots (base = frame_end(half)) end at 0x6C. The
//     whole routine frame sits in one bank, so no rounding is needed.
//     Everything fits.
//
// Expected (in = 3.0f; the exact emitted IR and the hand computation are
// traced in crates/driver/tests/float_e2e.rs):
//   out1 = 3.0/2.5          = 1.2        = 0x3F99999A  (fdiv call, RNE)
//   out2 = (float)(int)((3.0+0.25)*3.0)  = 9.0        = 0x41100000
//          (fadd+fmul exact: 3.25, 9.75; fptosi 9.75->9; sitofp 9->9.0)
//   s.c  = (3.0 < 0.75f)    = 0                       (fcmp olt)
//   s.f  = 1.0/3.0          = 0x3EAAAAAB              (fdiv, RNE)
//   out3 = pick(s)          = 0x3EAAAAAB              (byval/sret)
volatile float out1;
volatile float out2;
volatile float out3;
volatile float in;

struct S { unsigned char c; float f; };

__attribute__((noinline)) float half(float a) { return a / 2.5f; }

__attribute__((noinline)) float pick(struct S s) { return s.c ? 0.0f : s.f; }

__attribute__((noinline)) struct S mk(unsigned char c, float f) {
    struct S r;
    r.c = c;
    r.f = f;
    return r;
}

__attribute__((noinline)) float struct_step(unsigned char c, float rt) {
    struct S s = mk(c, rt);   // sret call + struct copy
    return pick(s);           // byval call
}

void main(void) {
    float a = in;                                // 3.0
    out1 = half(a);                              // fdiv call: 1.2
    out2 = (float)(int)((a + 0.25f) * 3.0f);     // fadd+fmul+fptosi+sitofp: 9.0
    out3 = struct_step((unsigned char)(a < 0.75f), 1.0f / a);  // cmp+sret+byval+fdiv: 0x3EAAAAAB
}
