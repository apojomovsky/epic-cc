// Pooled literal pointers reaching a callee that reads them through strlen
// (epic-cc#1026). `measure` is noinline so each pointer really crosses a call.
// Each result has its own global, so every call is observed.
#include <stdint.h>
#include <string.h>

static volatile uint8_t g_len_a;
static volatile uint8_t g_len_b;
static const char txt[] = "hello world";

__attribute__((noinline)) static uint8_t measure(const char *s) {
    return (uint8_t)strlen(s);
}

int main(void) {
    g_len_a = measure(txt);
    g_len_b = measure(txt + 6);
    return 0;
}
