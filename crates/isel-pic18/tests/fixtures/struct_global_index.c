// epic-cc#468: whole-struct copy into a runtime-indexed element of a
// global array, and the address of such an element taken as a value.
// Both shapes resolve a GEP over a global with a runtime index;
// isel-pic18 panicked taking that GEP's value
// ("cannot take the value of a GEP over Global").
//
// Expected: out == 0x9A. Hand trace (all arithmetic 8-bit wrapping):
//   put_copy(&c)          copies all 11 bytes into g_store[2]
//   put_addr(&c)          stores &g_store[2] into g_handle
//   g_handle->duty = 0x5A writes g_store[2].duty through that address
//   out = mode + cmp_lo + period_lo + duty + delay + src + pins
//       = 0x03 + 0x34 + 0xFF + 0x5A + 0x07 + 0x02 + 0x01 = 0x9A
struct Cfg {
    unsigned char inst;
    unsigned char mode;
    unsigned short cmp;
    unsigned short period;
    unsigned short duty;
    unsigned char delay;
    unsigned char src;
    unsigned char pins;
};

struct Cfg g_store[3];
struct Cfg *g_handle;
volatile unsigned char out;

__attribute__((noinline)) void put_copy(struct Cfg *h) {
    g_store[h->inst] = *h;
}
__attribute__((noinline)) void put_addr(struct Cfg *h) {
    g_handle = &g_store[h->inst];
}
void main(void) {
    struct Cfg c;
    c.inst = 2;
    c.mode = 0x03;
    c.cmp = 0x1234;
    c.period = 0x00FF;
    c.duty = 0x0010;
    c.delay = 0x07;
    c.src = 0x02;
    c.pins = 0x01;
    put_copy(&c);
    put_addr(&c);
    g_handle->duty = 0x5A;
    out = (unsigned char)(g_store[2].mode + (unsigned char)g_store[2].cmp
        + (unsigned char)g_store[2].period + (unsigned char)g_store[2].duty
        + g_store[2].delay + g_store[2].src + g_store[2].pins);
}
