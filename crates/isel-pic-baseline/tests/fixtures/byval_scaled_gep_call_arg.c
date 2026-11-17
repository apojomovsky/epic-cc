// epic-cc#1501: a scaled byval array element address passed
// directly as a call argument. The stride is 2, so the GEP value
// carries a single (2, idx) term over the byval slot.
// Baseline adaptation: get carries no noinline, so clang folds it
// into probe and the chain fits the 2-level hardware stack
// (main -> probe). probe stays noinline, so s stays a byval param.
// Expected: out == 0x5A (s.arr[2] low byte).
struct S { unsigned int arr[4]; };

volatile unsigned char out;
volatile unsigned char idx;

static unsigned char get(unsigned int *p) {
    return (unsigned char)*p;
}

__attribute__((noinline)) unsigned char probe(struct S s) {
    return get(&s.arr[idx]);
}

void main(void) {
    struct S s;
    s.arr[0] = 0x11;
    s.arr[1] = 0x22;
    s.arr[2] = 0x5A;
    s.arr[3] = 0x44;
    idx = 2;
    out = probe(s);
}
