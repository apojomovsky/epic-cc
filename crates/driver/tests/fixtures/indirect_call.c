// epic-cc#73 acceptance: calls through a function pointer, with the target
// selected at runtime. This is the HAL's callback shape: a callback pointer
// stored in a struct, then invoked through it. `sel` drives which callback
// is registered; the call result lands in the volatile `out` global and the
// machine halts.
//
//   sel == 0 -> out = f0() = 10
//   sel == 1 -> out = f1() = 20
//   sel == 2 -> out = f2() = 30
typedef unsigned char (*cb_t)(void);
struct dev {
    cb_t cb;
};
volatile unsigned char sel;
volatile unsigned char out;
struct dev g_dev;

unsigned char f0(void) { return 10; }
unsigned char f1(void) { return 20; }
unsigned char f2(void) { return 30; }

int main(void) {
    g_dev.cb = f0;
    if (sel == 1) g_dev.cb = f1;
    if (sel == 2) g_dev.cb = f2;
    out = g_dev.cb();
    return 0;
}
