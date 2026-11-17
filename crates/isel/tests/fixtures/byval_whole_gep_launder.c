// epic-cc#1501: laundering the address of a whole byval struct
// through uintptr_t. The byval slot IS the struct copy, so &s is
// the slot address. Expected: out == 0x5A (s.a).
typedef unsigned int uintptr_t;

struct S { unsigned char a; unsigned char b; };

volatile unsigned char out;
volatile uintptr_t laundered;

__attribute__((noinline)) unsigned char rd(struct S s) {
    laundered = (uintptr_t)&s;
    return *(unsigned char *)laundered;
}

void main(void) {
    struct S s;
    s.a = 0x5A;
    s.b = 0x11;
    out = rd(s);
}
