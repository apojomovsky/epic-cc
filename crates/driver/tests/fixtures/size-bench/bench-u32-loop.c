/* 32-bit counted loop with calls per iteration.
 * Menu-demo triage (epic-cc#617): epic_taskmgr_run (u32 tick loop with
 * three calls per iteration) is 457 vs XC8 37 words. The shape is a
 * wide counter plus opaque calls, forcing the full 32-bit
 * increment/compare on PIC18. */
typedef unsigned long uint32_t;

volatile uint32_t limit;
volatile unsigned char tick;

static void pump(void) { tick++; }
static void run_once(void) { tick += 2; }

void main(void) {
    uint32_t i;
    for (i = 0; i < limit; i++) {
        pump();
        run_once();
    }
}
