// XC8 tutorial blink for the PIC16F1937 (epic-cc#688): same surface as
// the PIC14 blink, on the Enhanced core. EPIC_CONFIG carries the clock
// until #691; see the PIC14 fixture.
#include <xc.h>
#include <epic-cc.h>

EPIC_CONFIG("osc=xt, xtal_hz=4000000, wdt=off");
#define _XTAL_FREQ 4000000

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
