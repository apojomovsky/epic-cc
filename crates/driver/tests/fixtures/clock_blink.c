// XC8 tutorial blink for the PIC16F877A (epic-cc#691): the acceptance
// program from the ticket. The crystal-less `#pragma config FOSC = XT`
// fixes no clock by itself; `_XTAL_FREQ` does, so each 500 ms delay is
// exactly 500000 instruction cycles.
#include <xc.h>

#pragma config FOSC = XT, WDTE = OFF, LVP = OFF

#define _XTAL_FREQ 4000000

volatile unsigned char done;

void main(void) {
    TRISB = 0x00;
    PORTBbits.RB0 = 0;
    __delay_ms(500);
    PORTBbits.RB0 = 1;
    __delay_ms(500);
    PORTBbits.RB0 = 0;
    done = PORTBbits.RB0;
}
