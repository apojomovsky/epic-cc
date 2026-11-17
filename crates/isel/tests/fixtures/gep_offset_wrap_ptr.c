// The indirect sibling of gep_offset_wrap.c: load and store through a
// pointer parameter whose total offset passes 255 (static 200 or 100
// plus a runtime term of 200, sums 400 and 300) (epic-cc#1466).
// Expected for in == 200: the store puts 0xA5 at big + 400 and
// out == 0x3C, the byte seeded at big + 300.
volatile unsigned short in;
volatile unsigned char out;
volatile unsigned char big[16];

__attribute__((noinline)) void wrap_store(unsigned char *p, unsigned char i) {
    p[200 + i] = 0xA5;
}

__attribute__((noinline)) unsigned char wrap_load(unsigned char *p, unsigned char i) {
    return p[100 + i];
}

void main(void) {
    unsigned char i = (unsigned char)in;
    wrap_store(big, i);
    out = wrap_load(big, i);
}
