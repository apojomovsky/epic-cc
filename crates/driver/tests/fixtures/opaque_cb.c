// epic-cc#137: a callback stored into an ISR-read global through an opaque
// runtime value (a struct-field load through a pointer, the HAL's
// `g_t0_overflow_cb = h->OverflowCallback` shape) cannot be resolved to a
// candidate. The ISR site compiles to a deterministic trap loop instead of
// panicking on the numeric register name (the pre-#137 behavior) or
// silently calling nothing. The `noinline` keeps the load opaque: clang
// cannot fold `h->cb` into a named function at the store site.

typedef void (*cb_t)(void);
struct dev {
    cb_t cb;
};
volatile struct dev g_dev;
volatile struct dev g_src;
volatile unsigned char out;
__attribute__((noinline)) void on_event(void) { out = 0x55; }
__attribute__((noinline)) void init(struct dev *h) { g_dev.cb = h->cb; }
__attribute__((interrupt(0))) void isr(void) {
    if (g_dev.cb) g_dev.cb();
}
void main(void) {
    g_src.cb = on_event;
    init(&g_src);
    out = 0x11;
}
