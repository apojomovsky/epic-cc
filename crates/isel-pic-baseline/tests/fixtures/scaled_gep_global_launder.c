// epic-cc#1501: laundering the address of a scaled array element
// of a global through uintptr_t. The GEP value carries a single
// (2, idx) term over a global base. Expected: out == 0x5A.
typedef unsigned int uintptr_t;

volatile unsigned int arr[4];
volatile unsigned char out;
volatile unsigned char idx;
volatile uintptr_t laundered;

void main(void) {
    arr[0] = 0x11;
    arr[1] = 0x22;
    arr[2] = 0x5A;
    arr[3] = 0x44;
    idx = 2;
    laundered = (uintptr_t)&arr[idx];
    out = *(unsigned char *)laundered;
}
