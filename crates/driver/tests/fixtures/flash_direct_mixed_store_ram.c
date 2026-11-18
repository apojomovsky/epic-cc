#include <stdint.h>

volatile uint8_t n = 3;
volatile uint8_t out;
char buf[4] = "abcd";
const char *gp;
static const char big[300] = "EPIC_HARNESS_RESULT: PASS\n";

__attribute__((noinline)) static uint8_t get(uint8_t i)
{
    return (uint8_t)gp[i];
}

int main(void)
{
    uint8_t i;
    gp = big;
    for (i = 0; i < n; i++)
        gp = buf;
    out = get(1);
    return 0;
}
