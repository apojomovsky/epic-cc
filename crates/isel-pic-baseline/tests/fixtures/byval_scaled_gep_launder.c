// epic-cc#1501: laundering the address of a scaled array element
// of a byval struct through uintptr_t. The element stride is 2, so
// the GEP value carries a single (2, idx) term over the byval slot.
// Expected: out == 0x5A (s.arr[2] low byte).
typedef unsigned int uintptr_t;

struct S { unsigned int arr[4]; };

volatile unsigned char out;
volatile unsigned char idx;
volatile uintptr_t laundered;

__attribute__((noinline)) unsigned char rd(struct S s) {
    laundered = (uintptr_t)&s.arr[idx];
    return *(unsigned char *)laundered;
}

void main(void) {
    struct S s;
    s.arr[0] = 0x11;
    s.arr[1] = 0x22;
    s.arr[2] = 0x5A;
    s.arr[3] = 0x44;
    idx = 2;
    out = rd(s);
}
