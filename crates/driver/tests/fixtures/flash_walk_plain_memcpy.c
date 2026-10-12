#include <stdint.h>
#include <string.h>

volatile uint8_t sent;
volatile uint8_t sum;
uint8_t buf[16];

int main(void)
{
    const char *p = "EPIC_HARNESS_RESULT: PASS\n";
    uint8_t total = 0;
    uint8_t count = 0;
    uint8_t n = 16;
    while (p[0] != '\0') {
        memcpy(buf, p, n);
        total += buf[0];
        p++;
        count++;
    }
    sum = total;
    sent = count;
    return 0;
}
