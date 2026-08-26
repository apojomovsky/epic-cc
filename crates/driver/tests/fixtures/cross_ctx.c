// epic-cc#137 acceptance: a peripheral callback stored by main-context code
// and invoked from an ISR through a global (the HAL's real shape:
// EPIC_TIMER0_Init stores into g_t0_overflow_cb, TIMER0_IRQHandler loads
// and invokes it). The callback is a shared function, so legalize duplicates
// it as `_isr` and main's store must reference the copy (which runs in the
// disjoint ISR region).
//
// The fixture distinguishes the copy from the original: main calls
// `on_event(0x10)` and the ISR fires while main's call is in flight (frame
// live). The ISR invokes the global with `in = 0x20`. If the ISR dispatched
// the main-context ORIGINAL, it would re-enter main's live frame and
// overwrite the param slot with 0x20, so main's call would return 0x21 and
// `out = r` would be 0x21, failing the assertion. The `_isr` copy runs in
// the disjoint ISR region and leaves main's frame intact: r stays 0x11.
//
// Hand computation (in = 0x20):
//   main: g_cb = on_event_isr; r = on_event(0x10) -> marker = 0x33, r = 0x11
//   <- ISR fires here (main's on_event frame live)
//   ISR:  out = g_cb(in) -> on_event_isr(0x20) -> out = 0x21
//   main: out = r -> 0x11; marker = 0x22
//   out == 0x11, marker == 0x22, halted.

typedef unsigned char (*cb_t)(unsigned char);
volatile cb_t g_cb;
volatile unsigned char out;
volatile unsigned char in;
volatile unsigned char marker;

__attribute__((noinline)) unsigned char on_event(unsigned char v) {
    marker = 0x33; /* inside main's on_event call (frame live) */
    return (unsigned char)(v + 1);
}

__attribute__((interrupt(0))) void isr(void) {
    if (g_cb) out = g_cb(in);
}
void main(void) {
    unsigned char r;
    g_cb = on_event;
    r = on_event(0x10); /* main's copy: r = 0x11, marker = 0x33 */
    out = r;            /* out = 0x11 */
    marker = 0x22;
}
