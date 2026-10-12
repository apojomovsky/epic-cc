// Pooled flash string table (epic-cc#815): several address-taken string
// literals through one consumer, so alloc flags them address-taken.
// Every member stages (the consumer reads through INDF), so no site
// reads through the pool and the `--const-pool` build carries no table
// (epic-cc#1088). `dup1`/`dup2` are distinct globals with identical
// bytes (C string literals with identical text merge in clang, so named
// arrays keep the dedup shape).
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
