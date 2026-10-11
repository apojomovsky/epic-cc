#include <stdint.h>

/* Two `const char *` walk phis that swap each iteration, one of them
 * advancing through a gep over the other (epic-cc#1117). iselcore refuses
 * the phi cycle as an unresolvable pointer chain; lifting that refusal
 * also needs a copy temp in the p18 backend (ADR-050). The volatile
 * initializer sets the trip count, so the loop is not folded away. */
volatile uint8_t count_in = 6;
volatile uint8_t out;
volatile uint8_t sum;

int main(void)
{
    const char *a = "EPIC_HARNESS_RESULT: PASS\n";
    const char *b = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
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
