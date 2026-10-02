/* CRC-16/CCITT over 32 bytes. A tight 8-iteration inner loop over a
 * shift-and-xor feedback: the canonical serial-protocol checksum the
 * bridge demo pays per frame. The buffer is seeded from a volatile
 * byte so the loop cannot fold. Plain C99: the private comparison
 * builds this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint8_t seed = 0x3C;
uint8_t buf[32];
volatile uint16_t crc;

void main(void) {
    uint8_t i;
    uint8_t k;
    uint16_t c = 0xFFFF;
    bench_mark = 1;
    for (i = 0; i < 32; i++) {
        buf[i] = (uint8_t)(seed + i * 7u);
    }
    for (i = 0; i < 32; i++) {
        c = (uint16_t)(c ^ ((uint16_t)buf[i] << 8));
        for (k = 0; k < 8; k++) {
            if (c & 0x8000u) {
                c = (uint16_t)((c << 1) ^ 0x1021u);
            } else {
                c = (uint16_t)(c << 1);
            }
        }
    }
    crc = c;
    bench_mark = 2;
}
