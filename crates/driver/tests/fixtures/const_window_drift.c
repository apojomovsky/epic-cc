#include <stdint.h>
#include "epic-cc.h"

EPIC_CONFIG("bor=on, osc=hs, lvp=off, pwrt=on, wdt=off, wrt=off, xtal_hz=20000000");

static const uint8_t t1[69] = {
    1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,
    35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69
};

volatile uint8_t g_idx;
volatile uint8_t g_out;

int main(void)
{
    if (g_idx == 0U) { g_out = (uint8_t)(t1[(g_idx + 0) & 7] + 0); }
    if (g_idx == 1U) { g_out = (uint8_t)(t1[(g_idx + 1) & 7] + 1); }
    if (g_idx == 2U) { g_out = (uint8_t)(t1[(g_idx + 2) & 7] + 2); }
    if (g_idx == 3U) { g_out = (uint8_t)(t1[(g_idx + 3) & 7] + 3); }
    if (g_idx == 4U) { g_out = (uint8_t)(t1[(g_idx + 4) & 7] + 4); }
    if (g_idx == 5U) { g_out = (uint8_t)(t1[(g_idx + 5) & 7] + 5); }
    if (g_idx == 6U) { g_out = (uint8_t)(t1[(g_idx + 6) & 7] + 6); }
    if (g_idx == 7U) { g_out = (uint8_t)(t1[(g_idx + 7) & 7] + 7); }
    if (g_idx == 8U) { g_out = (uint8_t)(t1[(g_idx + 8) & 7] + 8); }
    if (g_idx == 9U) { g_out = (uint8_t)(t1[(g_idx + 9) & 7] + 9); }
    if (g_idx == 10U) { g_out = (uint8_t)(t1[(g_idx + 10) & 7] + 10); }
    if (g_idx == 11U) { g_out = (uint8_t)(t1[(g_idx + 11) & 7] + 11); }
    if (g_idx == 12U) { g_out = (uint8_t)(t1[(g_idx + 12) & 7] + 12); }
    if (g_idx == 13U) { g_out = (uint8_t)(t1[(g_idx + 13) & 7] + 13); }
    if (g_idx == 14U) { g_out = (uint8_t)(t1[(g_idx + 14) & 7] + 14); }
    if (g_idx == 15U) {
        g_out = (uint8_t)(t1[(g_idx + 0) & 7] + 0);
        g_out = (uint8_t)(t1[(g_idx + 1) & 7] + 1);
        g_out = (uint8_t)(t1[(g_idx + 2) & 7] + 2);
        g_out = (uint8_t)(t1[(g_idx + 3) & 7] + 3);
        g_out = (uint8_t)(t1[(g_idx + 4) & 7] + 4);
        g_out = (uint8_t)(t1[(g_idx + 5) & 7] + 5);
        g_out = (uint8_t)(t1[(g_idx + 6) & 7] + 6);
        g_out = (uint8_t)(t1[(g_idx + 7) & 7] + 7);
        g_out = (uint8_t)(t1[(g_idx + 8) & 7] + 8);
        g_out = (uint8_t)(t1[(g_idx + 9) & 7] + 9);
        g_out = (uint8_t)(t1[(g_idx + 10) & 7] + 10);
        g_out = (uint8_t)(t1[(g_idx + 11) & 7] + 11);
        g_out = (uint8_t)(t1[(g_idx + 12) & 7] + 12);
        g_out = (uint8_t)(t1[(g_idx + 13) & 7] + 13);
    }
    return 0;
}
