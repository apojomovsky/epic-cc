#include <stdint.h>

/* RAM-pointer twin of `flash_walk_swap.c` (epic-cc#1141): two `char *`
 * walk phis swap each iteration, one advancing through a gep over the
 * other. Flash string literals stay p18-only on the PIC14 family (the
 * backends keep consts in flash with no RAM address), so this walks two
 * RAM buffers instead. The volatile initializer sets the trip count, so
 * the loop is not folded away. */
volatile uint8_t count_in = 6;
volatile uint8_t out;
volatile uint8_t sum;

char buf_a[8] = "ABCDEFG";
char buf_b[8] = "HJKLMNO";

int main(void)
{
    char *a = buf_a;
    char *b = buf_b;
    uint8_t acc = 0;
    uint8_t i;
    uint8_t n = count_in;
    for (i = 0; i < n; i++) {
        char *t = a;
        a = b;
        b = t + 1;
        acc = (uint8_t)(acc + (uint8_t)a[0]);
    }
    sum = acc;
    out = (uint8_t)b[0];
    return 0;
}
