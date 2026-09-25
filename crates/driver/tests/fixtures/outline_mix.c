/* Code-factoring differential (epic-cc#662): repeated address arithmetic,
 * bit tests, loops and identical function tails, with no const tables or
 * function pointers, so the factored program's RAM writes can be compared
 * write for write against the unfactored one. */
typedef unsigned char uint8_t;
typedef unsigned short uint16_t;

volatile uint8_t port;
uint8_t buf[24];
uint16_t acc;
uint8_t flags;

__attribute__((noinline)) static void mix_a(uint8_t i, uint8_t v) {
    buf[i] = (uint8_t)(buf[i] + v);
    acc = (uint16_t)(acc + buf[i]);
    if (flags & 0x01)
        port = buf[i];
    flags = (uint8_t)(flags ^ 0x03);
    acc = (uint16_t)(acc << 1);
    port = (uint8_t)acc;
}

__attribute__((noinline)) static void mix_b(uint8_t i, uint8_t v) {
    buf[i] = (uint8_t)(buf[i] ^ v);
    acc = (uint16_t)(acc + buf[i]);
    if (flags & 0x02)
        port = buf[i];
    flags = (uint8_t)(flags ^ 0x03);
    acc = (uint16_t)(acc << 1);
    port = (uint8_t)acc;
}

__attribute__((noinline)) static void mix_c(uint8_t i, uint8_t v) {
    buf[i] = (uint8_t)(buf[i] - v);
    acc = (uint16_t)(acc + buf[i]);
    if (flags & 0x04)
        port = buf[i];
    flags = (uint8_t)(flags ^ 0x03);
    acc = (uint16_t)(acc << 1);
    port = (uint8_t)acc;
}

void main(void) {
    uint8_t round, i;
    for (round = 0; round < 6; round++) {
        for (i = 0; i < 24; i++) {
            mix_a(i, (uint8_t)(round + i));
            mix_b((uint8_t)(23 - i), round);
            mix_c((uint8_t)((i * 5) % 24), (uint8_t)(i ^ round));
        }
        flags = (uint8_t)(flags + round);
        mix_a(round, flags);
        mix_b(round, (uint8_t)acc);
        mix_c(round, (uint8_t)(acc >> 8));
    }
    port = (uint8_t)(acc >> 8);
}
