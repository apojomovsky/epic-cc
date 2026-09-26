// XC8 tutorial blink for the PIC18F4550 (epic-cc#688): #pragma config on
// the internal oscillator, so no crystal frequency is needed to derive
// the clock. The oscillator-tree fields have no defaults, so every one
// is set explicitly (epic-cc#706 relaxes the omission later).
#include <xc.h>

#pragma config FOSC = INTOSCIO_EC, USBDIV = OFF, CPUDIV = DIV1, PLLDIV = NOPRESCALE, WDT = OFF, LVP = OFF, XINST = OFF

#define _XTAL_FREQ 8000000

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
