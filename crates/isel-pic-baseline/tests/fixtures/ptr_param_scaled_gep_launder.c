// epic-cc#1501: laundering the address of a scaled array element
// through a pointer parameter. The GEP value carries a single
// (2, idx) term over a slot that holds a runtime address.
// Expected: out == 0x5A (arr[2] low byte).
typedef unsigned int uintptr_t;

volatile unsigned int arr[4];
volatile unsigned char out;
volatile unsigned char idx;
volatile uintptr_t laundered;

__attribute__((noinline)) unsigned char rd(unsigned int *p) {
    laundered = (uintptr_t)&p[idx];
    return *(unsigned char *)laundered;
}

void main(void) {
    arr[0] = 0x11;
    arr[1] = 0x22;
    arr[2] = 0x5A;
    arr[3] = 0x44;
    idx = 2;
    out = rd(arr);
}
