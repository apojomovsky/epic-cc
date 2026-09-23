/* u16 decimal emission: repeated divide/modulo by 10 into a sink.
 * Menu-demo triage (epic-cc#617): the put_u16 cluster (decimal engine
 * inlined into a one-line wrapper) is 244 vs XC8 85 words, and the
 * heartbeat task repeats the shape six times per pass. */
typedef unsigned short uint16_t;

volatile uint16_t in;
volatile unsigned char digits[5];

void main(void) {
    uint16_t v = in;
    unsigned char i;
    for (i = 0; i < 5; i++) {
        digits[i] = (unsigned char)(v % 10U);
        v = (uint16_t)(v / 10U);
    }
}
