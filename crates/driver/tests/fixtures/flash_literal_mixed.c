// A flash literal and a RAM buffer reach the same non-inlined pointer
// parameter. The literal (27 bytes with NUL) is staged into RAM, so the
// callee's single INDF0 read serves both call sites.
#include <stdint.h>

volatile uint8_t out_flash;
volatile uint8_t out_ram;
char buf[4];

__attribute__((noinline)) static uint8_t at(const char *s, uint8_t i) {
    return (uint8_t)s[i];
}

int main(void) {
    buf[2] = 'Q';
    out_flash = at("EPIC_HARNESS_RESULT: PASS\n", 1);
    out_ram = at(buf, 2);
    return 0;
}
