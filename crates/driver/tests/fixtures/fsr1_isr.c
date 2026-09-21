// epic-cc#493 regression: an ISR that seeds FSR1 (an indirect-source
// memcpy) must not corrupt the preempted main context's in-flight FSR1
// copy pointer.
//
// main's `g_storage = *h` lowers to an indirect-source memcpy: FSR1 is
// seeded with the source pointer and read byte by byte, so the pointer is
// live across the copy's instructions. An interrupt inside that window,
// served by an ISR whose code also seeds FSR1, resumed main against the
// ISR's pointer before epic-cc#477 added FSR1L/FSR1H to every ISR
// prologue/epilogue. This fixture reproduces that window: the ISR copies
// its own struct, so its FSR1 seeding is reachable at any point.
typedef struct {
    unsigned short a;
    unsigned short b;
    unsigned char c;
    unsigned char (*cb)(void);
} Handle;

static Handle g_storage;
static unsigned char cb_impl(void) { return 0x55; }
volatile unsigned char g_out;

volatile Handle isr_src;
volatile Handle isr_dst;

__attribute__((interrupt(0))) void isr(void) {
    // An indirect-source copy, so the handler seeds FSR1 itself.
    isr_dst = isr_src;
}

__attribute__((noinline)) static void store_handle(Handle *h) {
    g_storage = *h;
}

int main(void) {
    Handle h;
    h.a = 0x1234;
    h.b = 0x5678;
    h.c = 0x9A;
    h.cb = cb_impl;
    store_handle(&h);
    g_out = g_storage.cb();
    return 0;
}
