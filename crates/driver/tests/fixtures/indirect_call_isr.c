// epic-cc#73 acceptance: an ISR fires a callback through a function pointer
// stored in a struct. The callback is a shared function (reachable from both
// main and the ISR), so legalize duplicates it as `_isr` and the ISR's stored
// pointer must reference the copy (which runs in the disjoint ISR region).
//
// `in` and `out` are volatile globals; PORTB is the F877A SFR at 0x06.
// The callback pointer is volatile so clang cannot fold the store+load into
// a direct call (the HAL's real pattern stores in one context and invokes in
// another, which clang cannot see through either).
//
// The e2e fires the interrupt mid-run and checks that the ISR's callback
// (the `_isr` copy) ran and wrote the expected value.

#define PORTB (*(volatile unsigned char *)0x06) // SFR access via inttoptr
typedef unsigned char (*cb_t)(unsigned char);
struct dev {
    cb_t volatile cb;
};
volatile unsigned char out;
volatile unsigned char in;
struct dev g_dev;

__attribute__((noinline)) unsigned char on_event(unsigned char v) {
    return (unsigned char)(v + 1);
}

__attribute__((interrupt(0))) void isr(void) {
    PORTB = 0x55;         // SFR write from the ISR
    g_dev.cb = on_event;  // store the callback (rewritten to on_event_isr)
    out = g_dev.cb(in);   // invoke it through the pointer
}

void main(void) {
    out = in;             // e.g. in = 0x10
    PORTB = 0x11;         // SFR write from main
    out = on_event(out);  // main's copy of the shared callback
    PORTB = 0x22;
}
