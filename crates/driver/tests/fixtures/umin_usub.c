// llvm.umin / llvm.usub.sat lowering (epic-cc#160): clang -O1 emits
// llvm.umin for a priority min-finding loop over a memory array and
// llvm.usub.sat for a guarded decrement of a memory value (the reload
// defeats the guard). Both must lower to correct code and run.

#include <stdint.h>

struct slot { uint8_t prio; uint8_t flags; };
static struct slot g_slots[4];
volatile uint8_t g_min;
volatile uint8_t g_count;

int main(void) {
    g_slots[0].prio = 3; g_slots[1].prio = 1; g_slots[2].prio = 2; g_slots[3].prio = 0;
    g_slots[0].flags = 1; g_slots[1].flags = 1; g_slots[2].flags = 1; g_slots[3].flags = 1;
    uint8_t best = 0xFF;
    for (uint8_t i = 0; i < 4; i++) {
        if (g_slots[i].flags & 1u) {
            uint8_t p = g_slots[i].prio;
            if (p < best) best = p;
        }
    }
    g_min = best;
    g_count = 2;
    if (g_count == 0u) {
        g_count = 0u;
    } else {
        g_count--;
    }
    return 0;
}
