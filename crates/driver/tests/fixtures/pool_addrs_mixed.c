// Pool addresses and copy gating (epic-cc#816): the direct-arg string
// shape mirrors the encoder-sim log strings, plus one const used both
// directly and through a dynamic index. With `--const-pool` the direct
// consts keep no RAM copy and materialize as pool addresses, while the
// mixed const keeps its copy. The consumer reads no bytes itself (a
// byte loop here would inline and LSR-chase over the literals, a RAM
// use until epic-cc#811); the sim behavior proof lands in #817.
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
