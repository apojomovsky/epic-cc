/* Struct-array scan. Eight records walked for one field each: the
 * non-power-of-two stride is a shift-add chain re-emitted per access
 * on PIC14 (see bench-struct-scan). One element is poked from a
 * volatile seed inside the region so the walk cannot fold. Plain
 * C99: the private comparison builds this file unchanged. */
#include <stdint.h>

typedef struct {
    uint16_t id;
    uint8_t tag;
    uint8_t val;
} item_t;

volatile unsigned char bench_mark;
volatile uint8_t seed = 0x07;
item_t items[8];
volatile uint16_t total;

void main(void) {
    uint8_t i;
    uint16_t sum = 0;
    bench_mark = 1;
    items[3].val = seed;
    for (i = 0; i < 8; i++) {
        sum = (uint16_t)(sum + items[i].id + items[i].val);
    }
    total = sum;
    bench_mark = 2;
}
