// epic-cc#1464: a GEP passed directly as a call argument has no slot
// of its own on the baseline backend: the allocator folds it into the
// call site. Passing it must materialize the address, not look the
// GEP up as a slot. Expected: in == 3 -> out == buf[3] == 0x5A.
volatile unsigned char buf[8];
volatile unsigned char in;
volatile unsigned char out;

__attribute__((noinline)) unsigned char get(unsigned char *p) {
    return *p;
}

void main(void) {
    buf[3] = 0x5A;
    out = get(buf + (in & 3));
}
