// epic-cc#196 regression: a source that guards `#include <stdio.h>`
// behind a condition the active build never takes (epic-hal's own
// `#ifndef __EPIC_CC__` pattern) must not trigger the driver's injected
// stdio runtime, which needs a `putchar` this file never defines.
#ifndef __EPIC_CC__
#include <stdio.h>
#endif

volatile int g_out;

void main(void)
{
    g_out = 1;
    for (;;) {
    }
}
