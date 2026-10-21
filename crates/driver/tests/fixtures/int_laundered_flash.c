#include <stdint.h>

/* A flash address that passes through the integer domain keeps its
 * space (epic-cc#1346). The table's address is copied into a RAM
 * pointer slot, read back as an integer, and converted back to a
 * pointer: the dereference must read program memory, not the RAM
 * address that happens to share the slot. */
static const char table[] = "EPIC_HARNESS_RESULT: PASS\n";

volatile uint8_t out;
volatile uint8_t sent;

/* The address as an integer, laundered through a global and a local.
 * The compiler proves the space statically, so the pointer reads
 * flash on every path. */
static uintptr_t laundered;

int main(void)
{
    const char *p = table;
    uintptr_t addr = (uintptr_t)p;
    laundered = addr;

    const char *q = (const char *)laundered;
    int len = 0;
    while (q[len] != '\0') {
        out = (uint8_t)q[len];
        len++;
    }
    sent = (uint8_t)len;
    return 0;
}
