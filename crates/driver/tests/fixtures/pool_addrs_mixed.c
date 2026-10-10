// Pool addresses and copy gating (epic-cc#816): the direct-arg string
// shape mirrors the encoder-sim log strings, plus one const used both
// directly and through a dynamic index. With `--const-pool` the mixed
// const keeps its RAM copy. `consume` hands its pointer to strlen, which
// reads the bytes, so the direct literals stage RAM copies too (epic-cc#1026).
#include <stdint.h>
#include <string.h>

static volatile uint8_t g_sink;
static volatile uint8_t g_idx;

static void consume(const char *s) {
    g_sink = (uint8_t)strlen(s);
}

static const char mixed[] = "mixed-bytes-payload";

int main(void) {
    consume("alpha");
    consume("bravo");
    consume(mixed);
    consume(&mixed[g_idx]);
    return 0;
}
