// epic-cc#125 acceptance: a whole-struct copy of a multi-byte handle (the
// HAL's `g_t2_storage = *h` pattern) lowers to `load i64`/`store i64`
// (PIC14, unpacked) or `llvm.memcpy` with an indirect source (PIC18,
// packed). Both must compile and run: the copied callback pointer must
// dispatch through the storage copy.
//
//   store_handle(&h) -> g_storage = *h (7 bytes)
//   g_out = g_storage.cb() -> 0x55
typedef struct {
    unsigned short a;
    unsigned short b;
    unsigned char c;
    unsigned char (*cb)(void);
} Handle;
static Handle g_storage;
static unsigned char cb_impl(void) { return 0x55; }
volatile unsigned char g_out;
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
