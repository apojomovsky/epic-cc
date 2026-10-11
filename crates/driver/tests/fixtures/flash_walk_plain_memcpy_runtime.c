#include <stdint.h>
#include <string.h>

volatile uint8_t len_in;
uint8_t buf[16];

int main(void)
{
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t n = len_in;
    while (p[0] != '\0') {
        memcpy(buf, p, n);
        p++;
    }
    return 0;
}
