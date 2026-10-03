// Dispatch-lane polarity guard (epic-cc#806): two flag lanes sharing one
// snapshot, one `eq` (runs when the bit is clear) and one `ne` (runs when
// the bit is set). A polarity-inverted bit-test collapse runs the wrong
// lane each way, so the markers below pin both directions.
#include <stdint.h>

volatile uint8_t g_flags;
volatile uint8_t g_seen_eq;
volatile uint8_t g_seen_ne;

static void on_eq_clear(void) { g_seen_eq = 0xA1; }
static void on_ne_set(void) { g_seen_ne = 0xB2; }

static void dispatch_snapshot(void) {
    uint8_t snap = g_flags;
    if ((snap & 0x04u) == 0u) on_eq_clear();
    if ((snap & 0x08u) != 0u) on_ne_set();
}

int main(void) {
    dispatch_snapshot();
    return 0;
}
