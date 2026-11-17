// epic-cc#1501: laundering the address of a scaled array element
// at a non-zero field offset through a pointer parameter. The GEP
// value carries a single (2, idx) term plus the constant field
// offset of arr, over a slot that holds a runtime address.
// Expected: out == 0x5A (s.arr[2] low byte).
typedef unsigned int uintptr_t;

struct S { unsigned int head; unsigned int arr[4]; };

volatile struct S s;
volatile unsigned char out;
volatile unsigned char idx;
volatile uintptr_t laundered;

__attribute__((noinline)) unsigned char rd(struct S *p) {
    laundered = (uintptr_t)&p->arr[idx];
    return *(unsigned char *)laundered;
}

void main(void) {
    s.head = 0x99;
    s.arr[0] = 0x11;
    s.arr[1] = 0x22;
    s.arr[2] = 0x5A;
    s.arr[3] = 0x44;
    idx = 2;
    out = rd((struct S *)&s);
}
