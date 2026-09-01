// epic-cc#152 acceptance: an indirect call site must only collect
// candidates with the matching argument count. The 1-arg RB-style site
// (`g_cb1(0x55)`) must NOT collect the 0-arg `on_a` callback, or isel
// panics copying the i8 arg into a param-less callee's slots.
//
// `g_cb0`/`g_cb1` are volatile globals so clang cannot fold the stores and
// loads into direct calls. Each callback writes its own effect global, so
// the e2e can assert both sites dispatched: out1 = 0x55 (on_b) and
// out2 = 1 (on_a).
typedef void (*cb0_t)(void);
typedef void (*cb1_t)(unsigned char);
volatile cb0_t g_cb0;
volatile cb1_t g_cb1;
volatile unsigned char out1;
volatile unsigned char out2;
static void on_a(void) { out2 = 1; }
static void on_b(unsigned char v) { out1 = v; }
__attribute__((interrupt(0))) void isr(void) {
    if (g_cb1) g_cb1(0x55);
    if (g_cb0) g_cb0();
}
void main(void) {
    g_cb1 = on_b;
    g_cb0 = on_a;
}
