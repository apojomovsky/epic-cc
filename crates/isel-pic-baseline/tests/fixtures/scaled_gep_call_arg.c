// epic-cc#1501: a scaled GEP passed directly as a call argument.
// The stride is 2, so the GEP value carries a single (2, idx) term.
// Expected: out == 0x5A (arr[2] low byte).
volatile unsigned int arr[4];
volatile unsigned char out;
volatile unsigned char idx;

__attribute__((noinline)) unsigned char get(unsigned int *p) {
    return (unsigned char)*p;
}

void main(void) {
    arr[0] = 0x11;
    arr[1] = 0x22;
    arr[2] = 0x5A;
    arr[3] = 0x44;
    idx = 2;
    out = get(&arr[idx]);
}
