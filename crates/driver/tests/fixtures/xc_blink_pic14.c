// XC8 tutorial blink for the PIC16F877A (epic-cc#688): the device header
// from <xc.h>, the _XTAL_FREQ delay spelling, and PORTBbits bit access.
// mid and done pin the bit reads; the delays dominate the cycle count.
// Configuration rides on EPIC_CONFIG until #691 derives the clock from
// _XTAL_FREQ; the header surface is the tutorial one either way.
#define _XTAL_FREQ 4000000

#include <xc.h>
#include <epic-cc.h>

EPIC_CONFIG("osc=xt, xtal_hz=4000000, wdt=off, lvp=off");

volatile unsigned char mid;
volatile unsigned char done;

void main(void) {
    TRISB = 0x00;
    PORTBbits.RB0 = 0;
    __delay_ms(1);
    PORTBbits.RB0 = 1;
    mid = PORTBbits.RB0;
    __delay_ms(1);
    PORTBbits.RB0 = 0;
    done = PORTBbits.RB0;
}
