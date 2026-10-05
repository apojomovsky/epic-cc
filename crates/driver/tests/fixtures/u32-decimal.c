/* u32 decimal emission through the shared digit helper (epic-cc#722).
 * Value-driven digit loop storing ASCII digits into a global buffer: the
 * shape clang inlines into every put_u16/put_idec copy, which legalize
 * shares as one __udec_u32 call on PIC18. */
typedef unsigned long uint32_t;

volatile uint32_t in;
volatile unsigned char n;
unsigned char buf[12];

void main(void) {
    uint32_t v = in;
    unsigned char i = 0;
    do {
        buf[i] = (unsigned char)('0' + (v % 10u));
        v /= 10u;
        i++;
    } while (v != 0u);
    n = i;
}
