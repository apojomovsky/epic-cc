// CC-3 acceptance: EPIC_AT places `out` at a fixed address; EPIC_CONFIG
// sets a real, checkable fuse combination; EPIC_FOSC_HZ is read back into
// a global so the test can assert the driver derived it correctly.
#include <epic-cc.h>

EPIC_CONFIG("osc=xt, xtal_hz=4000000, wdt=off, lvp=off");

volatile unsigned char out EPIC_AT(0x0021);
unsigned long fosc = EPIC_FOSC_HZ;

void main(void) {
    out = 0x2A;
}
