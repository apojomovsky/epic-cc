// epic-cc#1421: a pointer laundered through `uintptr_t` on PIC18.
//
// The cast lowers as a same-width `trunc i16 -> i16`: the pointer and the
// integer are both 16 bits, so the cast is a copy. The backend used to
// refuse it ("trunc must narrow"), which aborted the compile.
//
// Two source shapes reach that trunc today, and each one materializes its
// address differently:
//   1. a pointer LOADED from a RAM global (the address bytes live in the
//      global's slot),
//   2. the address of a LOCAL (an alloca: the address is the slot's own
//      frame address, a compile-time constant).
//
// The laundered address goes through a VOLATILE global and is read back
// through a pointer cast from it, so the cast cannot be folded away and
// the round trip reads the byte AT the laundered address: a wrong address
// reads the wrong byte. The test proves the address, not just that the
// cast compiled.
//
// The address of a GLOBAL in value position (`(uintptr_t)&g`) is a
// constant-expression `ptrtoint`, which the frontend does not parse yet
// (epic-cc#1422); that shape is out of scope here.
//
// Expected: out == 0x5A, the byte `g` holds, read back through the
// laundered address of `g`; and out2 == 0x33, the byte at the laundered
// address of the local, proving the local's address is the local's own
// slot and not some other frame address.
typedef unsigned int uintptr_t;

volatile unsigned char g = 0x5A;
volatile unsigned char out;
volatile unsigned char out2;

volatile uintptr_t laundered;
volatile uintptr_t laundered_local;

void main(void) {
    /* Shape 1: a pointer loaded from a RAM global. */
    const unsigned char *volatile p = (const unsigned char *)&g;
    laundered = (uintptr_t)p;
    out = *(const unsigned char *)laundered;

    /* Shape 2: the address of a local. */
    unsigned char local = 0x33;
    laundered_local = (uintptr_t)&local;
    out2 = *(unsigned char *)laundered_local;
}
