// A flash literal over 255 bytes and a RAM buffer reach the same
// non-inlined pointer parameter. Consts over 255 bytes get no RAM copy,
// so the mixed call shape has no address to serve: a precise user error.
#include <stdint.h>

volatile uint8_t out_flash;
volatile uint8_t out_ram;
char buf[4];

__attribute__((noinline)) static uint8_t at(const char *s, uint8_t i) {
    return (uint8_t)s[i];
}

int main(void) {
    buf[2] = 'Q';
    out_flash = at("EPIC_HARNESS_RESULT: PASS\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", 1);
    out_ram = at(buf, 2);
    return 0;
}
