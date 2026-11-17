// Two runtime terms on a loaded base, compared as an icmp operand:
// `base + ~(in & 1) + in` is a slotless GEP value with two terms.
// With in == 8 the first term is 255, so byte 0 carries on the first
// add only (255 + base_lo wraps, then + 8 does not), and the true
// end is base + 263. The cursor starts at base + 258, so the bounded
// loop runs 5 times and lands on the end: out == 5, out2 == 1. A
// carry taken from the final low-byte add loses the first carry and
// puts the end one page low, so the bound trips instead.
volatile unsigned char arr[64];
volatile unsigned short in;
volatile unsigned char out;
volatile unsigned char out2;

struct H {
    unsigned char a, b;
    unsigned char *p;
};
volatile struct H h;

void main(void)
{
    unsigned char *base;
    unsigned char *end;
    unsigned char *cur;
    unsigned char n = 0;

    h.p = arr;
    base = h.p;
    end = base + (unsigned char)~(in & 1) + in;
    cur = base + (unsigned char)~(in & 1) + (in - 5);
    while (cur != end && n < 20) {
        n++;
        cur++;
    }
    out = n;
    out2 = (cur == end);
}
