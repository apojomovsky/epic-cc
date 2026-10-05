// Pool-reader log loop (epic-cc#817): the mdb `epic_harness_log` shape
// (byte loop over a pointer arg) with direct, select, and RAM call
// sites. `noinline` keeps the calls surviving clang: inlined loops over
// literals are the const-chase shape (epic-cc#811), not this ticket.
// The never-taken select arms pad the pool past 255 bytes so it spans
// two chunks; the logged shorts then land past the boundary and the
// per-byte chunk-id dispatch is exercised, while the transcript itself
// stays small enough for one bank window.
#include <stdint.h>

static volatile uint8_t g_tx[80];
static volatile uint8_t g_len;
static volatile uint8_t g_sel;

static void putc(char c) {
    g_tx[g_len++] = (uint8_t)c;
}

__attribute__((noinline)) void epic_harness_log(const char *fmt, ...) {
    while (*fmt) {
        putc(*fmt);
        fmt++;
    }
}

static char g_buf[16];

int main(void) {
    epic_harness_log(g_sel ? "pad one, one hundred twenty bytes of never-logged pool padding...................."
                           : "one\n");
    epic_harness_log(g_sel ? "pad two, one hundred twenty bytes of never-logged pool padding...................."
                           : "two\n");
    epic_harness_log(g_sel ? "pad three, one hundred twenty bytes of never-logged pool padding.................."
                           : "three\n");
    epic_harness_log("four\n");
    for (uint8_t i = 0; i < 15; i++) {
        g_buf[i] = (char)('A' + (i % 26));
    }
    g_buf[15] = '\0';
    epic_harness_log(g_buf);
    return 0;
}
