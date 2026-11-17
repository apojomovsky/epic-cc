#include <string.h>
#include <stdint.h>
#include <stddef.h>

volatile uint8_t out;
volatile char buf[8];
volatile char src[4];

void *memset(void *s, int c, size_t n) {
    unsigned char *p = (unsigned char *)s;
    for (size_t i = 0; i < n; i++) p[i] = (unsigned char)(c + (int)i);
    return s;
}

void main(void) {
    src[0] = 'a'; src[1] = 'b'; src[2] = '\0';
    memset((void *)buf, 10, 4);
    strcpy((char *)buf, (const char *)src);
    out = (uint8_t)(buf[3] + (uint8_t)strlen((const char *)buf));
}
