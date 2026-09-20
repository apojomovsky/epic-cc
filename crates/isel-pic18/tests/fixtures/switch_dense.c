// epic-cc#479: two dense switches lowered to PCL jump tables, driven
// through the real simulator. `dispatch` is the base-0 i8 shape;
// `dispatch2` is a nonzero-base i8 switch (padding entries, low-bound
// reject). Every case body calls a distinct noinline helper (the real
// dispatch shape, as in the IRQ handlers): clang cannot collapse the
// switch into a const lookup table, so it must survive into the IR
// for the table lowering to see it. NOTE: per-case global side
// effects beyond the switched value do not survive wholeprog on
// switch-shaped code (epic-cc#497); the observable contract here is
// the dispatched value.
unsigned char out;

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
        case 0: out = h0(k); break;
        case 1: out = h1(k); break;
        case 2: out = h2(k); break;
        case 3: out = h3(k); break;
        case 4: out = h4(k); break;
        case 5: out = h5(k); break;
        case 6: out = h6(k); break;
        case 7: out = h7(k); break;
        default: out = 99; break;
    }
}

unsigned char out2;

__attribute__((noinline)) static unsigned char g0(unsigned char k) { return k + 40; }
__attribute__((noinline)) static unsigned char g1(unsigned char k) { return k * 2; }
__attribute__((noinline)) static unsigned char g2(unsigned char k) { return k ^ 0x33; }
__attribute__((noinline)) static unsigned char g3(unsigned char k) { return 200 - k; }
__attribute__((noinline)) static unsigned char g4(unsigned char k) { return k + 9; }
__attribute__((noinline)) static unsigned char g5(unsigned char k) { return k | 0x80; }
__attribute__((noinline)) static unsigned char g6(unsigned char k) { return k + 200; }

void dispatch2(unsigned char k) {
    switch (k) {
        case 4: out2 = g0(k); break;
        case 5: out2 = g1(k); break;
        case 6: out2 = g2(k); break;
        case 7: out2 = g3(k); break;
        case 8: out2 = g4(k); break;
        case 9: out2 = g5(k); break;
        case 10: out2 = g6(k); break;
        default: out2 = 7; break;
    }
}
unsigned char results[9];
unsigned char results2[8];

int main(void) {
    for (unsigned char k = 0; k < 9; k = k + 1) {
        dispatch(k);
        results[k] = out;
    }
    for (unsigned char k = 3; k < 11; k = k + 1) {
        dispatch2(k);
        results2[k - 3] = out2;
    }
    return 0;
}
