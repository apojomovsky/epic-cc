// P5 (PIC18) interrupt-gate acceptance: a program that MASKS interrupts
// with INTCON, so the simulator's GIE / enable-bit modelling is what
// decides when the handler runs.
//
// This is PIC14's interrupt_gate.c with INTCON's address changed from
// 0x0B (the F877A bank-mirrored SFR) to 0xFF2 (the PIC18F4550's INTCON).
// The bit layout is identical: bit 7 = GIE, bit 4 = INT0IE, bit 1 =
// INT0IF. Everything else is byte-identical.
//
// The e2e requests an interrupt while `stage == 1`, i.e. with INT0IE set
// but GIE still clear. A simulator that ignores INTCON would vector
// immediately; the modelled one latches the request and takes it only
// after main writes GIE at stage 2. `stage` makes each window observable
// from the test.
//
// Expected: isr_ran == 0 for the whole masked window, then exactly 1 after
// GIE goes up. Exactly one, not more: the handler never clears INT0IF, so
// a simulator that re-armed on the still-set flag would spin in the handler
// and never reach the final stage.
#define INTCON (*(volatile unsigned char *)0xFF2)

volatile unsigned char isr_ran;
volatile unsigned char stage;

__attribute__((interrupt(0))) void isr(void) {
    isr_ran = (unsigned char)(isr_ran + 1);
}

void main(void) {
    stage = 1;      // the test requests the interrupt in this window
    INTCON = 0x10;  // INT0IE set, GIE clear: enabled source, still masked
    stage = 2;      // still masked here
    INTCON = 0x90;  // GIE | INT0IE: unmasked, the pending request is taken
    stage = 3;      // reached only if the handler ran once and returned
}
