#include <stdint.h>

volatile uint8_t out_flash;
volatile uint8_t out_ram;
char buf[4] = "abcd";
static const char big[200] = "EPIC_HARNESS_RESULT: PASS\n";
const char *fg;

__attribute__((noinline)) static uint8_t get(const char *p, uint8_t i)
{
    return (uint8_t)p[i];
}

int main(void)
{
    fg = big;
    out_flash = get(big, 1);
    out_ram = get(buf, 1);
    return 0;
}
