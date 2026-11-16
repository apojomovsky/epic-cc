// A multi-term pointer argument whose GEP index is a va_arg value.
// The width lookup for the term must see the va_arg definition, and
// its high byte must join the sum. Expected for in == 1 and the
// variadic argument 5: out == 0x5A.
volatile unsigned char in;
volatile unsigned char out;
volatile unsigned char buf[16];
volatile unsigned char big[16];
__attribute__((noinline)) static void put(unsigned char *p, unsigned char v) { *p = v; }
__attribute__((noinline)) static void varput(int n, ...) {
    __builtin_va_list ap;
    __builtin_va_start(ap, n);
    unsigned int t = __builtin_va_arg(ap, unsigned int);
    __builtin_va_end(ap);
    unsigned char sel = (unsigned char)(in & 1);
    unsigned char *p = (sel == 0) ? buf + 3 : buf + 2;
    unsigned char *q = p + t;
    put(q, 0x5A);
}
void main(void) {
    put(big + 290, 0x11);
    varput(0, 5);
    out = buf[7];
}
