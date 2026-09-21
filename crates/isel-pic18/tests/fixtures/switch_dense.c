// epic-cc#479: two dense switches lowered to PCL jump tables, driven
// through the real simulator. `dispatch` is the base-0 i8 shape;
// `dispatch2` is a nonzero-base i8 switch (padding entries, low-bound
// reject). Every case body calls a distinct noinline helper (the real
// dispatch shape, as in the IRQ handlers): clang cannot collapse the
// switch into a const lookup table, so it must survive into the IR
// for the table lowering to see it. Each case also writes a second
// global (`aux`), so the sim assertions below pin per-case side effects
// alongside the dispatched value: a dispatch that reaches the right
// trampoline but drops the case body's second store fails them. That is
// the shape epic-cc#497 reported vanishing; these assertions are its
// permanent check.
unsigned char out;
unsigned char aux;

__attribute__((noinline)) static unsigned char h0(unsigned char k) { return k + 10; }
__attribute__((noinline)) static unsigned char h1(unsigned char k) { return k * 2 + 1; }
__attribute__((noinline)) static unsigned char h2(unsigned char k) { return k ^ 0x5A; }
__attribute__((noinline)) static unsigned char h3(unsigned char k) { return k + 100; }
__attribute__((noinline)) static unsigned char h4(unsigned char k) { return 200 - k; }
__attribute__((noinline)) static unsigned char h5(unsigned char k) { return k + 7; }
__attribute__((noinline)) static unsigned char h6(unsigned char k) { return k | 0x80; }
__attribute__((noinline)) static unsigned char h7(unsigned char k) { return k + 200; }

void dispatch(unsigned char k) {
    switch (k) {
        case 0: out = h0(k); aux = 1; break;
        case 1: out = h1(k); aux = 2; break;
        case 2: out = h2(k); aux = 3; break;
        case 3: out = h3(k); aux = 4; break;
        case 4: out = h4(k); aux = 5; break;
        case 5: out = h5(k); aux = 6; break;
        case 6: out = h6(k); aux = 7; break;
        case 7: out = h7(k); aux = 8; break;
        default: out = 99; aux = 9; break;
    }
}

unsigned char out2;
unsigned char aux2;

__attribute__((noinline)) static unsigned char g0(unsigned char k) { return k + 40; }
__attribute__((noinline)) static unsigned char g1(unsigned char k) { return k * 2; }
__attribute__((noinline)) static unsigned char g2(unsigned char k) { return k ^ 0x33; }
__attribute__((noinline)) static unsigned char g3(unsigned char k) { return 200 - k; }
__attribute__((noinline)) static unsigned char g4(unsigned char k) { return k + 9; }
__attribute__((noinline)) static unsigned char g5(unsigned char k) { return k | 0x80; }
__attribute__((noinline)) static unsigned char g6(unsigned char k) { return k + 200; }

void dispatch2(unsigned char k) {
    switch (k) {
        case 4: out2 = g0(k); aux2 = 11; break;
        case 5: out2 = g1(k); aux2 = 12; break;
        case 6: out2 = g2(k); aux2 = 13; break;
        case 7: out2 = g3(k); aux2 = 14; break;
        case 8: out2 = g4(k); aux2 = 15; break;
        case 9: out2 = g5(k); aux2 = 16; break;
        case 10: out2 = g6(k); aux2 = 17; break;
        default: out2 = 7; aux2 = 19; break;
    }
}
unsigned char results[9];
unsigned char results2[8];
// The second global each case writes, captured per iteration so the sim
// can assert it. Before epic-cc#497 was investigated these stores were
// believed to vanish; if they ever do, these arrays read back stale.
unsigned char markers[9];
unsigned char markers2[8];

int main(void) {
    for (unsigned char k = 0; k < 9; k = k + 1) {
        dispatch(k);
        results[k] = out;
        markers[k] = aux;
    }
    for (unsigned char k = 3; k < 11; k = k + 1) {
        dispatch2(k);
        results2[k - 3] = out2;
        markers2[k - 3] = aux2;
    }
    return 0;
}
