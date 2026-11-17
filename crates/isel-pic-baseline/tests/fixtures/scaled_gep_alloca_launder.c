// epic-cc#1501: laundering the address of a scaled array element
// of a local object through uintptr_t. The element stride is 2, so
// the GEP value carries a single (2, idx) term over the alloca slot.
// Expected: out == 0x5A (arr[2] low byte).
typedef unsigned int uintptr_t;

volatile unsigned char out;
volatile unsigned char idx;
volatile uintptr_t laundered;

__attribute__((noinline)) unsigned char rd(void) {
    unsigned int arr[4];
    arr[0] = 0x11;
    arr[1] = 0x22;
    arr[2] = 0x5A;
    arr[3] = 0x44;
    laundered = (uintptr_t)&arr[idx];
    return *(unsigned char *)laundered;
}

void main(void) {
    idx = 2;
    out = rd();
}
