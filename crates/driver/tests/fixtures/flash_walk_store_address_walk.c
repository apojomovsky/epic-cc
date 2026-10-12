#include <stdint.h>
volatile uint8_t out;
const char *volatile g;
int main(void) {
    const char *p = "ABCD";
    for (uint8_t i = 0; i < 2; i++) p = p + 1;
    g = p;
    out = (uint8_t)g[0];
    return 0;
}
