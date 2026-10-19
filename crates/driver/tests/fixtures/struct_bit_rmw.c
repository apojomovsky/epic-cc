// Struct-spelled single-bit SFR writes fuse to BSF/BCF (epic-cc#1281).
// Final state: PORTB = 0x1A (RB1, RB3, RB4 set; RB0 and RB2 set then
// cleared; the multi-bit mask 0xC0 keeps the byte path and sets RB6/RB7
// then clears RB6), TRISB = 0xFF (the bank-1 struct write sets the
// already-set TRISB2: TRIS POR is all-inputs), INTCON = 0x80,
// out = PORTB read back after the writes.
#include <xc.h>
#include <epic-cc.h>

EPIC_CONFIG("osc=xt, xtal_hz=4000000, wdt=off, lvp=off");

volatile unsigned char out;

void main(void) {
    PORTBbits.RB0 = 1;
    PORTBbits.RB1 = 1;
    PORTBbits.RB2 = 1;
    PORTBbits.RB0 = 0;
    PORTBbits.RB2 = 0;
    PORTB |= 0x08;
    PORTB |= 0x10;
    PORTB |= 0xC0;
    PORTB &= ~0x40;
    PORTB &= 0x7F;
    TRISBbits.TRISB2 = 1;
    INTCONbits.GIE = 1;
    out = PORTB;
}
