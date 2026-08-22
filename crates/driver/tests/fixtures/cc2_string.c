#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
#include <string.h>

volatile uint8_t in;
volatile uint8_t out;

volatile char src[8];
volatile char dst[8];
volatile char dst2[8];
volatile char dst3[8];
volatile char buf[8];
volatile char tail[2];
volatile char five[6];
volatile char needle_cde[4];

void main(void) {
    uint8_t a = in;
    bool flag = true;
    size_t n = 3;

    src[0] = 'a'; src[1] = 'b'; src[2] = 'c'; src[3] = '\0';
    tail[0] = 'd'; tail[1] = '\0';
    five[0] = 'a'; five[1] = 'b'; five[2] = 'c'; five[3] = 'd'; five[4] = 'e'; five[5] = '\0';
    needle_cde[0] = 'c'; needle_cde[1] = 'd'; needle_cde[2] = 'e'; needle_cde[3] = '\0';

    memset((void*)dst, 0, 8);
    memcpy((void*)dst, (const void*)src, n);

    uint8_t acc = 0;
    acc += (uint8_t)strlen((const char*)dst);                       /* 3 */
    acc += (strcmp((const char*)dst, (const char*)src) == 0);       /* 1 */
    acc += (memcmp((const void*)dst, (const void*)src, n) == 0);    /* 1 */
    acc += (uint8_t)strnlen((const char*)dst, 8);                   /* 3 */

    strncpy((char*)dst2, (const char*)src, 3);
    dst2[3] = '\0';
    acc += (strncmp((const char*)dst2, (const char*)src, 3) == 0);  /* 1 */

    strncat((char*)dst2, (const char*)tail, 1);
    acc += (dst2[3] == 'd');                                        /* 1 */

    memset((void*)dst3, 0, 8);
    strcpy((char*)dst3, (const char*)src);
    strcat((char*)dst3, (const char*)tail);
    acc += (dst3[3] == 'd');                                        /* 1 */
    acc += (strlen((const char*)dst3) == 4);                        /* 1 */

    /* memmove with overlapping ranges must copy back-to-front. */
    strcpy((char*)buf, (const char*)five);
    memmove((void*)&buf[1], (const void*)buf, 3);
    acc += (buf[0] == 'a' && buf[1] == 'a' && buf[2] == 'b'
            && buf[3] == 'c' && buf[4] == 'e');                     /* 1 */

    acc += flag ? 1 : 0;                                            /* 1 */
    acc += (n == 3);                                                /* 1 */
    acc += a;                                                       /* in */

    acc += (memchr((const void*)five, 'c', 5) != (void*)0);         /* 1 */
    acc += (strchr((const char*)five, 'd') != (char*)0);            /* 1 */
    acc += (strrchr((const char*)five, 'b') != (char*)0);           /* 1 */
    acc += (strstr((const char*)five, (const char*)needle_cde) != (char*)0);          /* 1 */

    /* for in=7, acc = 3+1+1+3+1+1+1+1+1+1+1+7+4 = 26 */
    out = acc;
}
