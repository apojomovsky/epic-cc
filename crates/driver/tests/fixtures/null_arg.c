// Null function argument (epic-cc#144): clang prints a NULL pointer
// argument as `ptr noundef null`, which irparse's call-arg whitelist
// used to reject. The callback is stored into a global and dispatched
// only when non-null; main passes NULL and the guarded path returns 7.

#include <stdint.h>

static void (*g_cb)(uint8_t);

static void cb(uint8_t v) { (void)v; }

static uint8_t call_cb(uint8_t (*fn)(uint8_t)) {
    if (fn == 0) return 7;
    return fn(3);
}

volatile uint8_t g_out;

int main(void) {
    g_cb = cb;
    g_out = call_cb(0);
    return 0;
}
