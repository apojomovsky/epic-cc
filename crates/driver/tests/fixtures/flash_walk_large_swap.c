#include <stdint.h>

/* Swap walk over two flash strings too long for a RAM copy, loading
 * through the walk inside the loop: the walk slots hold flash addresses
 * that INDF would read as RAM (epic-cc#1197, epic-cc#1203). */
#define T10 "0123456789"
#define T100 T10 T10 T10 T10 T10 T10 T10 T10 T10 T10

static const char big_a[] = T100 T100 T100 "end";
static const char big_b[] = T100 T100 T100 "fin";

volatile uint8_t count_in = 6;
volatile uint8_t out;
volatile uint8_t sum;

int main(void)
{
    const char *a = big_a;
    const char *b = big_b;
    uint8_t acc = 0;
    uint8_t i;
    uint8_t n = count_in;
    for (i = 0; i < n; i++) {
        const char *t = a;
        a = b;
        b = t + 1;
        acc = (uint8_t)(acc + (uint8_t)a[0]);
    }
    sum = acc;
    out = (uint8_t)b[0];
    return 0;
}
