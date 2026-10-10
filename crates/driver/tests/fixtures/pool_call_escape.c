// Pooled literal pointers passed to a call that is not inlined (epic-cc#1026):
// the callee dereferences them with RAM semantics, so each const keeps its RAM
// copy under `--const-pool`. `noinline` keeps the call alive at -O1.
#include <stdint.h>

static volatile uint8_t g_direct;
static volatile uint8_t g_offset;
static const char txt[] = "hello";

__attribute__((noinline)) static uint8_t pick(const char *p) {
    return (uint8_t)p[2];
}

int main(void) {
    g_direct = pick(txt);
    g_offset = pick(txt + 1);
    return 0;
}
