// Pooled flash string table (epic-cc#815): several address-taken string
// literals through one consumer, so alloc flags them address-taken and
// the `--const-pool` build concatenates them into `__const_pool`.
// `dup1`/`dup2` are distinct globals with identical bytes (C string
// literals with identical text merge in clang, so named arrays carry
// the dedup proof): the pool must carry those bytes once.
#include <stdint.h>
#include <string.h>

static volatile uint8_t g_sink;

static void consume(const char *s) {
    size_t n = strlen(s);
    for (size_t i = 0; i < n; i++) {
        g_sink = (uint8_t)s[i];
    }
}

static const char dup1[] = "same-bytes";
static const char dup2[] = "same-bytes";

int main(void) {
    consume("alpha");
    consume("bravo longer");
    consume("alpha");
    consume("charlie delta echo foxtrot");
    consume(dup1);
    consume(dup2);
    return 0;
}
