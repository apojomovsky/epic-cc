// epic-cc#1417: a switch that irparse preserves as a dense run with a
// residual chain behind it. The preserved run's case target carries a
// phi, and the residual edge's incoming must move to the residual
// chain block while the preserved run's edges keep naming the switch
// block.
//
// The shape is a Duff's device: the cases 0..5 form the dense run and
// the case 100 is the leftover the residual chain handles.

#include <stdint.h>

volatile uint8_t out[8];
volatile uint8_t in[8];
volatile uint8_t sel;

void copy(volatile uint8_t *to, const volatile uint8_t *from, uint8_t count)
{
    switch (sel) {
    case 0: case 1: case 2: case 3: case 4: case 5: case 100:
        do { *to++ = *from++;
    case 6:  *to++ = *from++;
    case 7:  *to++ = *from++;
        } while (--count > 0);
        break;
    }
}

int main(void)
{
    sel = 100;
    copy(out, in, 3);
    return 0;
}
