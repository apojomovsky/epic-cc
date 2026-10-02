/* 32-byte memcpy. The source is seeded from a volatile byte inside
 * the timed region so its contents are unknown and the call cannot
 * fold; the checksum sinks the destination. Plain C99: the private
 * comparison builds this file unchanged. */
#include <stdint.h>
#include <string.h>

volatile unsigned char bench_mark;
volatile uint8_t seed = 0x5A;
unsigned char src[32];
unsigned char dst[32];
volatile uint8_t checksum;

void main(void) {
    uint8_t i;
    uint8_t sum = 0;
    bench_mark = 1;
    for (i = 0; i < 32; i++) {
        src[i] = (unsigned char)(seed + i);
    }
    memcpy(dst, src, 32);
    for (i = 0; i < 32; i++) {
        sum = (uint8_t)(sum + dst[i]);
    }
    checksum = sum;
    bench_mark = 2;
}
