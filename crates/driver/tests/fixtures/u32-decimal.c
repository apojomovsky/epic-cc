/* u32 decimal emission through the shared digit helper (epic-cc#722).
 * Value-driven digit loops storing ASCII digits into global buffers: the
 * shape clang inlines into every put_u16/put_idec copy, which legalize
 * shares as one __udec_u32 call on PIC18. Two loops, so the sharing pays
 * for the helper body (a lone loop stays expanded). */
typedef unsigned long uint32_t;

volatile uint32_t in;
volatile unsigned char n;
unsigned char buf[12];
unsigned char buf2[12];

void main(void) {
    uint32_t v = in;
    unsigned char i = 0;
    do {
        buf[i] = (unsigned char)('0' + (v % 10u));
        v /= 10u;
        i++;
    } while (v != 0u);
    n = i;
    v = in;
    i = 0;
    do {
        buf2[i] = (unsigned char)('0' + (v % 10u));
        v /= 10u;
        i++;
    } while (v != 0u);
}
