/* 32-byte memset. The fill byte is volatile so the store run cannot
 * fold; the checksum sinks the buffer. Plain C99: the private
 * comparison builds this file unchanged. */
#include <stdint.h>
#include <string.h>

volatile unsigned char bench_mark;
volatile uint8_t fill = 0xA5;
unsigned char buf[32];
volatile uint8_t checksum;

void main(void) {
    uint8_t i;
    uint8_t sum = 0;
    bench_mark = 1;
    memset(buf, fill, 32);
    for (i = 0; i < 32; i++) {
        sum = (uint8_t)(sum + buf[i]);
    }
    checksum = sum;
    bench_mark = 2;
}
