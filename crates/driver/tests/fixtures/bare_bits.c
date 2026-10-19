// Bare bit acceptance (epic-cc#707): tutorial `GIE = 1`-style spellings
// drive SFR bits as real entities. Final state pins every shape:
// INTCON = 0x80 (GIE set), PORTB = 0x02 (RB1 struct-set survives the
// bare RB0 toggle and clear), TRISB = 0xFF (the bank-1 entity sets the
// already-set TRISB1: TRIS POR is all-inputs), out = 0
// (branch on the cleared RB0) plus PORTB (0x02).
#include <xc.h>
#include <epic-cc.h>

EPIC_CONFIG("osc=xt, xtal_hz=4000000, wdt=off, lvp=off");

volatile unsigned char flag;
volatile unsigned char out;

void main(void) {
    GIE = 1;
    RB0 = 1;
    PORTBbits.RB1 = 1;
    RB0 = 0;
    TRISB1 = 1;
    RB0 ^= 1;
    RB0 = flag;
    if (RB0) {
        out = 1;
    } else {
        out = 0;
    }
    out += PORTB;
}
