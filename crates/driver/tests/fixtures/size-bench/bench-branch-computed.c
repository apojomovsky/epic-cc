/* Branch on a just-computed byte: the reload before the branch is
 * dead when the producer already set Z (epic-cc#668). The flag test
 * and the countdown retest below are the struct-scan shapes that
 * carry this pattern on the menu demo. */
typedef unsigned char uint8_t;

volatile uint8_t g_in;
volatile uint8_t g_out;

void main(void) {
    uint8_t a = g_in;
    if ((uint8_t)(a & 3u)) {
        g_out = 1;
    } else {
        g_out = 2;
    }
    if ((uint8_t)(a + 5u)) {
        g_out = (uint8_t)(g_out + 1u);
    }
}
