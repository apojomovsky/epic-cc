// epic-cc#137: the param-forwarded registration shape (the HAL's
// `EPIC_GPIO_RegisterChangeCallback(on_rb_change)` pattern): main passes the
// callback as a call argument, the callee stores the param into a global
// the ISR reads. legalize rewrites the call-site argument to the `_isr`
// copy, and isel materializes the function address as LOW/HIGH literals in
// the untyped ptr arg path. The e2e fires the interrupt mid-run and checks
// the ISR's callback (the `_isr` copy) ran.
//
// Hand computation:
//   main: register(on_event_isr); out = 0x11
//   <- ISR fires here
//   ISR:  if (g_cb) g_cb() -> on_event_isr: out = 0x55
//   main: __start SLEEP halts the machine
//   out == 0x55, halted.

typedef void (*cb_t)(void);
volatile cb_t g_cb;
volatile unsigned char out;
__attribute__((noinline)) void on_event(void) { out = 0x55; }
__attribute__((noinline)) void register_cb(cb_t cb) { g_cb = cb; }
__attribute__((interrupt(0))) void isr(void) {
    if (g_cb) g_cb();
}
void main(void) {
    register_cb(on_event);
    out = 0x11;
}
