/* Counted flash-to-RAM copy loop with a twice-used index (epic-cc#937):
 * the BUILD_LINE macro in the menu demo copies a flash literal into the
 * shared line buffer one byte per iteration, then space-fills the tail.
 * Each loop reads its index for the memory offset and for the bump, so
 * the index phi feeds more than one use and the epic-cc#767 single-use
 * gate stays shut; the epic-cc#937 latch-region check folds the bump
 * into the phi's home anyway. */
typedef unsigned char uint8_t;
typedef unsigned short uint16_t;

static const char lit[] = "Brightness:";
static char buf[16];
volatile unsigned char sink;

void main(void) {
    uint16_t i;
    for (i = 0; i < 12U; i++) {
        buf[i] = lit[i];
    }
    for (; i < 16U; i++) {
        buf[i] = ' ';
    }
    buf[12] = '\0';
    sink = (unsigned char)buf[0];
}
