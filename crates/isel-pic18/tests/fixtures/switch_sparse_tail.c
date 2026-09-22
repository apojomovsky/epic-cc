// epic-cc#578: dense run 0..5 with a sparse 200 tail and default. The
// run lowers to a PCL jump table, the tail stays a compare chain behind
// the table default. Every case body calls a distinct noinline helper
// (the real dispatch shape, as in the IRQ handlers): clang cannot
// collapse the switch into a const lookup, so the split lowering must
// survive into isel. `main` drives every table entry, the holes around
// the tail (6, 100, 199, 201), the sparse hit (200), and the far
// default (255).
unsigned char out;
unsigned char aux;

__attribute__((noinline)) static unsigned char h0(unsigned char k) { return k + 10; }
__attribute__((noinline)) static unsigned char h1(unsigned char k) { return k * 2 + 1; }
__attribute__((noinline)) static unsigned char h2(unsigned char k) { return k ^ 0x5A; }
__attribute__((noinline)) static unsigned char h3(unsigned char k) { return k + 100; }
__attribute__((noinline)) static unsigned char h4(unsigned char k) { return 200 - k; }
__attribute__((noinline)) static unsigned char h5(unsigned char k) { return k + 7; }
__attribute__((noinline)) static unsigned char hs(unsigned char k) { return k ^ 0x5A; }

void dispatch(unsigned char k) {
    switch (k) {
        case 0: out = h0(k); aux = 1; break;
        case 1: out = h1(k); aux = 2; break;
        case 2: out = h2(k); aux = 3; break;
        case 3: out = h3(k); aux = 4; break;
        case 4: out = h4(k); aux = 5; break;
        case 5: out = h5(k); aux = 6; break;
        case 200: out = hs(k); aux = 7; break;
        default: out = 99; aux = 8; break;
    }
}

unsigned char results[12];
unsigned char markers[12];

int main(void) {
    dispatch(0); results[0] = out; markers[0] = aux;
    dispatch(1); results[1] = out; markers[1] = aux;
    dispatch(2); results[2] = out; markers[2] = aux;
    dispatch(3); results[3] = out; markers[3] = aux;
    dispatch(4); results[4] = out; markers[4] = aux;
    dispatch(5); results[5] = out; markers[5] = aux;
    dispatch(6); results[6] = out; markers[6] = aux;
    dispatch(100); results[7] = out; markers[7] = aux;
    dispatch(199); results[8] = out; markers[8] = aux;
    dispatch(200); results[9] = out; markers[9] = aux;
    dispatch(201); results[10] = out; markers[10] = aux;
    dispatch(255); results[11] = out; markers[11] = aux;
    return 0;
}
