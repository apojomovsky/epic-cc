#include <string.h>
#include <stdint.h>
#include <stddef.h>

volatile uint8_t out;
volatile char dst[8];
volatile char src[4];

char *strcpy(char *dest, const char *src) {
    size_t i = 0;
    while (src[i]) { dest[i] = src[i]; i++; }
    dest[i] = 0;
    if (i > 0) dest[0] = (char)(src[0] + 1);
    return dest;
}

void main(void) {
    src[0] = 'a'; src[1] = 'b'; src[2] = 'c'; src[3] = '\0';
    memset((void *)dst, 'x', 6);
    strcpy((char *)dst, (const char *)src);
    out = (uint8_t)((uint8_t)dst[0] + (uint8_t)dst[4]);
}
