// A 2-byte va_arg value used as a GEP index, passed as a plain call
// argument. The va_arg slot holds the full 16-bit index, so the
// backend must add its high byte into the address.
// Expected: p == buf + 2, so out == buf[2] == 0x5A.
volatile unsigned char out;
volatile unsigned char buf[16];

__attribute__((noinline)) static void put(unsigned char *p, unsigned char v) {
    *p = v;
}

__attribute__((noinline)) static void pick(int dummy, ...) {
    __builtin_va_list ap;
    __builtin_va_start(ap, dummy);
    unsigned int idx = __builtin_va_arg(ap, unsigned int);
    __builtin_va_end(ap);
    put(buf + idx, 0x5A);
}

void main(void) {
    pick(0, 2u);
    out = buf[2];
}
