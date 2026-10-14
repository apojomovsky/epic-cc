// Negative: a bare bit has no address (epic-cc#707). Naming `&GIE` must
// fail with the rule, never silently materialize a byte address.
#include <xc.h>
#include <epic-cc.h>

EPIC_CONFIG("osc=xt, xtal_hz=4000000, wdt=off, lvp=off");

volatile unsigned char *p;

void main(void) {
    p = &GIE;
    *p = 1;
}
