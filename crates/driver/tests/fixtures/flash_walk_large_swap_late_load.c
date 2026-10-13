#include <stdint.h>

/* Swap walk over two flash strings too long for a RAM copy, with every
 * load after the loop: the back-edge copies form a cycle over flash
 * addresses before any load through a walk slot is emitted (epic-cc#1141,
 * epic-cc#1203). */
#define T10 "0123456789"
#define T100 T10 T10 T10 T10 T10 T10 T10 T10 T10 T10

static const char big_a[] = T100 T100 T100 "end";
static const char big_b[] = T100 T100 T100 "fin";

volatile uint8_t count_in = 6;
volatile uint8_t out;

int main(void)
{
    const char *a = big_a;
    const char *b = big_b;
    uint8_t i;
    uint8_t n = count_in;
    for (i = 0; i < n; i++) {
        const char *t = a;
        a = b;
        b = t + 1;
    }
    out = (uint8_t)b[0];
    return 0;
}
