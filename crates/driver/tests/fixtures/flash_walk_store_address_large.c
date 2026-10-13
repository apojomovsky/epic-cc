#include <stdint.h>
/* A 300-byte const whose address is stored into RAM: too large for a RAM
 * copy, so the store is still refused (epic-cc#1223 boundary). */
volatile uint8_t out;
static const uint8_t big[300] = { 1 };
const uint8_t *volatile g;
int main(void)
{
    g = big;
    out = g[0];
    return 0;
}
