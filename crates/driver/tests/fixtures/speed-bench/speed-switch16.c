/* 16-case switch dispatch. The selector is volatile so the dispatch
 * cannot fold to one arm; each arm does a distinct widening add so
 * arms cannot merge. Menu-demo triage (epic-cc#617) found the event
 * switch on this shape. Plain C99: the private comparison builds
 * this file unchanged. */
#include <stdint.h>

volatile unsigned char bench_mark;
volatile uint8_t sel = 9;
volatile uint16_t acc;

void main(void) {
    uint16_t a = 0;
    bench_mark = 1;
    switch (sel) {
    case 0: a = 0x0011u; break;
    case 1: a = 0x0023u; break;
    case 2: a = 0x0045u; break;
    case 3: a = 0x0089u; break;
    case 4: a = 0x0111u; break;
    case 5: a = 0x0223u; break;
    case 6: a = 0x0445u; break;
    case 7: a = 0x0889u; break;
    case 8: a = 0x1111u; break;
    case 9: a = 0x2223u; break;
    case 10: a = 0x4445u; break;
    case 11: a = 0x8889u; break;
    case 12: a = 0x1112u; break;
    case 13: a = 0x2224u; break;
    case 14: a = 0x4446u; break;
    case 15: a = 0x8888u; break;
    default: a = 0; break;
    }
    acc = a;
    bench_mark = 2;
}
