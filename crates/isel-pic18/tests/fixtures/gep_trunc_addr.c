// epic-cc#1426: a runtime-indexed address cast to an integer.
//
// `clang` lowers the pointer-to-integer cast as `ptrtoint`, which `irparse`
// turns into a same-width `trunc i16 -> i16` over the GEP. The GEP's base
// is a global and its index is a runtime value, so the address materializes
// as the base literal plus the index (the `[(1, reg)]` term of the folded
// base), not as a plain slot copy.
//
// The laundered address is read back through a pointer cast from it, so a
// wrong address reads the wrong byte: the test proves the address, not
// just that the cast compiled.
//
// Expected: out2 == buf[idx], the byte at the laundered address, proving
// the runtime index rode the address; and out == 0x5A, the byte `g` holds,
// read back through the laundered address of a plain pointer.
typedef unsigned int uintptr_t;

volatile unsigned char g = 0x5A;
volatile unsigned char buf[4] = {0x11, 0x22, 0x33, 0x44};
volatile unsigned char idx = 2;
volatile unsigned char out;
volatile unsigned char out2;

volatile uintptr_t laundered_indexed;
volatile uintptr_t laundered_plain;

void main(void) {
    laundered_indexed = (uintptr_t)&buf[idx];
    out2 = *(volatile unsigned char *)laundered_indexed;

    const unsigned char *volatile p = &g;
    laundered_plain = (uintptr_t)p;
    out = *(const unsigned char *)laundered_plain;
}
