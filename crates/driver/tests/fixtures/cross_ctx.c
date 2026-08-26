// epic-cc#137 acceptance: a peripheral callback stored by main-context code
// and invoked from an ISR through a global (the HAL's real shape:
// EPIC_TIMER0_Init stores into g_t0_overflow_cb, TIMER0_IRQHandler loads
// and invokes it). The callback is a shared function, so legalize duplicates
// it as `_isr` and main's store must reference the copy (which runs in the
// disjoint ISR region). The e2e fires the interrupt mid-run and checks the
// ISR's callback ran and wrote the expected value.
//
// Hand computation:
//   main: g_cb = on_event_isr; out = 0x11
//   <- ISR fires here
//   ISR:  if (g_cb) g_cb() -> on_event_isr: out = 0x55
//   main: __start SLEEP halts the machine
//   out == 0x55, halted.

#define PORTB (*(volatile unsigned char *)0x06) // SFR access via inttoptr
typedef void (*cb_t)(void);
volatile cb_t g_cb;
volatile unsigned char out;
__attribute__((noinline)) void on_event(void) { out = 0x55; }

__attribute__((interrupt(0))) void isr(void) {
    if (g_cb) g_cb();
}
void main(void) {
    g_cb = on_event;
    out = 0x11;
}
