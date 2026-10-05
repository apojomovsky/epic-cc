// epic-cc#844: two staged strings followed by an indexed 48-byte table.
// The section model carries each staged routine's post-banking length, so
// a later table's window fold runs at the true base address.
#include <stdint.h>

static volatile uint8_t g_buf[33];
static volatile uint8_t g_out;
static volatile uint8_t g_probe;
static volatile uint8_t g_idx;

__attribute__((noinline)) static void sink(const char *s) {
    for (uint8_t i = 0; i < 33; i++) {
        g_buf[i] = (uint8_t)s[i];
    }
}

static const char msg[] = "0123456789ABCDEF0123456789ABCDEF";
static const char msg2[] = "GHIJKLMNOPQRSTUVWXYZ012345ABCDEF";
static const uint8_t tab[48] = {
    0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,
    24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47
};

int main(void)
{
    sink(msg);
    g_probe = g_buf[5];
    sink(msg2);
    g_out = tab[(g_idx + 0) & 47];
    g_out = tab[(g_idx + 1) & 47];
    g_out = tab[(g_idx + 2) & 47];
    g_out = tab[(g_idx + 3) & 47];
    g_out = tab[(g_idx + 4) & 47];
    g_out = tab[(g_idx + 5) & 47];
    g_out = tab[(g_idx + 6) & 47];
    g_out = tab[(g_idx + 7) & 47];
    g_out = tab[(g_idx + 8) & 47];
    g_out = tab[(g_idx + 9) & 47];
    return 0;
}
