// P5 (PIC18) interrupt acceptance: SFR access via inttoptr, a noinline
// shared helper duplicated for the ISR, and the ISR fired mid-run by the
// simulator.
//
// This is PIC14's interrupt.c with PORTB's address changed from 0x06 (the
// F877A bank-mirrored SFR) to 0xF81 (the PIC18F4550's PORTB). Everything
// else is byte-identical: the INTCON bit game stays out of this fixture
// (the e2e fires the interrupt directly via Pic18::fire_interrupt, exactly
// like the PIC14 interrupt_e2e test).
//
// `in` and `out` are volatile globals (their addresses come from the alloc
// layout the driver used); PORTB is the PIC18F4550 SFR at absolute address
// 0xF81 (`*(volatile unsigned char *)0xF81` -> clang's `inttoptr (i16
// 3969 to ptr)`).
//
// The program is shaped so clang -O1 keeps every op:
//   out = in;                      out = 0x10 (in = 0x10)
//   PORTB = 0x11;                  SFR write from main
//   out = bump(out);               main's copy of the shared helper
//   out = (unsigned char)(out + 1);
//   out = (unsigned char)(out + bump(2));
//   PORTB = 0x22;
//
// The ISR writes PORTB = 0x55 and calls the shared bump() (legalize's
// duplication rewrites that call to the `_isr` copy, so the ISR context
// never enters main's copy). bump() is add-only (no mul/div), so the ISR
// stays clear of the runtime routines' scratch.
//
// The e2e fires the interrupt at a traced pc (the injection point is
// hand-computed from the exact emitted IR, as the PIC14 test does), the
// ISR preempts main before the shared helper's argument is read. Hand
// computation (in = 0x10):
//   main: out = in                        -> 0x10
//   main: PORTB = 0x11
//   <- ISR fires here
//   ISR:  PORTB = 0x55; out = bump_isr(out = 0x10) -> 0x11
//   main: out = bump(out = 0x11)          -> 0x12
//   main: out = out + 1                   -> 0x13
//   main: out = out + bump(2)             -> 0x13 + 3 = 0x16
//   main: PORTB = 0x22
//   out == 0x16, PORTB == 0x22, halted (the no-interrupt run gives 0x15,
//   the ISR's bump is observable in the final out).
// The test recomputes this from the exact emitted IR + the injection point.

#define PORTB (*(volatile unsigned char *)0xF81) // SFR access via inttoptr
volatile unsigned char out;
volatile unsigned char in;

__attribute__((noinline)) unsigned char bump(unsigned char x) { return (unsigned char)(x + 1); }

__attribute__((interrupt(0))) void isr(void) {
    PORTB = 0x55;         // SFR write from the ISR
    out = bump(out);      // shared helper (duplicated for the ISR)
}

void main(void) {
    out = in;                             // e.g. in = 0x10
    PORTB = 0x11;                         // SFR write from main
    out = bump(out);                      // shared helper (main's copy)
    out = (unsigned char)(out + 1);       // <- the interrupt fires during this stretch
    out = (unsigned char)(out + bump(2));
    PORTB = 0x22;
}
