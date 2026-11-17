// epic-cc#1464: laundering the address of a byval struct field
// through uintptr_t. A byval param's slot IS the struct copy, like
// an alloca, so taking its field address must materialize the slot
// address plus the field offset. The laundered address goes through
// a volatile global and is read back through a pointer cast, so the
// cast cannot be folded away. Expected: out == 0x5A (s.b).
typedef unsigned int uintptr_t;

struct S { unsigned char a; unsigned char b; };

volatile unsigned char out;
volatile uintptr_t laundered;

__attribute__((noinline)) unsigned char rd(struct S s) {
    laundered = (uintptr_t)&s.b;
    return *(unsigned char *)laundered;
}

void main(void) {
    struct S s;
    s.a = 0x11;
    s.b = 0x5A;
    out = rd(s);
}
