// Milestone-7 structs acceptance: the full M7 surface in one program.
//   - sret call + struct copy: `g = mk(3, 0x1234)` — mk returns `struct Pair`
//     through a hidden sret pointer; the caller memcpy's the result into the
//     volatile global `g`.
//   - byval call from a global: `sum(g)` passes `g` as a byval `struct Pair`
//     (callee reads p.a at offset 0, p.b at offset 2).
//   - dynamic array-in-struct: `pick(arr)` reads `x.v[x.n]` — an FSR/INDF
//     load with a runtime index inside a byval struct; `arr.v[arr.n] = 0x11`
//     is the matching dynamic store into the global array field.
//   - nested-struct field math: `go.in.a / go.in.b / go.z` — folded byte
//     GEPs off one volatile global (offsets 0, 2, 4).
//
// NOTE: the brief's draft used a *local* `struct Outer o; o.in.a = 1; ...`
// but clang -O1 SROA's that away entirely (the whole expression folds to
// `out += 6`), losing the nested-struct GEP coverage. `volatile struct Outer`
// locals get scalarized into per-field allocas by SROA, also losing the
// GEPs. A volatile global struct cannot be folded at all, so `go` keeps the
// identical hand-computable value (1 + 2 + 3 = 6) while the nested GEPs
// survive in the IR. Same semantic coverage, same expected out.
//
// Expected: out == 0x4E for the fixed inputs. Hand trace (8-bit wraps):
//   g = mk(3, 0x1234)              -> Pair{a=3, b=0x1234}
//   out = sum(g) = 3 + 0x34        -> 0x37   (b truncated to uchar by the
//                                     cast: 3 + 0x1234 = 0x1237, low byte 0x37)
//   arr.n = 2; arr.v[2] = 0x5A; arr.v[arr.n] = 0x11  -> v[2] ends 0x11
//   out = 0x37 + pick(arr) = 0x37 + 0x11     -> 0x48
//   go.in.a = 1; go.in.b = 2; go.z = 3; out = 0x48 + 1 + 2 + 3 -> 0x4E
struct Pair  { unsigned char a; unsigned short b; };
struct A     { unsigned char n; unsigned char v[4]; };
struct Outer { struct Pair in; unsigned char z; };

volatile unsigned char out;
volatile struct Pair g;
volatile struct A    arr;
volatile struct Outer go;

__attribute__((noinline)) unsigned char sum(struct Pair p) {      // byval
    return (unsigned char)(p.a + p.b);
}
__attribute__((noinline)) unsigned char pick(struct A x) {        // byval + dynamic array-in-struct
    return x.v[x.n];
}
__attribute__((noinline)) struct Pair mk(unsigned char a, unsigned short b) {  // sret
    struct Pair r; r.a = a; r.b = b; return r;
}
void main(void) {
    g = mk(3, 0x1234);                    // sret call + struct copy (memcpy)
    out = sum(g);                         // byval from a global: 3 + 0x34 = 0x37
    arr.n = 2; arr.v[2] = 0x5A; arr.v[arr.n] = 0x11;             // dynamic struct-array store
    out = (unsigned char)(out + pick(arr));                     // 0x37 + 0x11 = 0x48
    go.in.a = 1; go.in.b = 2; go.z = 3;                         // nested structs (folded byte GEPs)
    out = (unsigned char)(out + go.in.a + go.in.b + go.z);      // 0x48 + 1 + 2 + 3 = 0x4E
}
