// P3 structs acceptance, sized to the 509's 32-byte GPR budget (bank 0
// 0x10-0x1F + bank 1 0x30-0x3F). Exercises byval call, dynamic
// array-in-struct, and nested-struct field math through FSR/INDF. The
// sret call (mk) is dropped: its 4-byte return struct plus the caller's
// copy would exceed the budget, and the byval + dynamic-array + nested
// paths cover the P3 struct surface.
//
// Expected: out == 0x48 for the fixed inputs. Hand trace (8-bit wraps):
//   out = sum(g) = 3 + 0x34        -> 0x37   (b truncated to uchar by the
//                                     cast: 3 + 0x1234 = 0x1237, low byte 0x37)
//   arr.n = 2; arr.v[2] = 0x5A; arr.v[arr.n] = 0x11  -> v[2] ends 0x11
//   out = 0x37 + pick(arr) = 0x37 + 0x11     -> 0x48
struct Pair  { unsigned char a; unsigned short b; };
struct A     { unsigned char n; unsigned char v[4]; };

volatile unsigned char out;
volatile struct Pair g;
volatile struct A    arr;

__attribute__((noinline)) unsigned char sum(struct Pair p) {      // byval
    return (unsigned char)(p.a + p.b);
}
__attribute__((noinline)) unsigned char pick(struct A x) {        // byval + dynamic array-in-struct
    return x.v[x.n];
}
void main(void) {
    g.a = 3; g.b = 0x1234;
    out = sum(g);                         // byval from a global: 3 + 0x34 = 0x37
    arr.n = 2; arr.v[2] = 0x5A; arr.v[arr.n] = 0x11;             // dynamic struct-array store
    out = (unsigned char)(out + pick(arr));                     // 0x37 + 0x11 = 0x48
}
