// epic-cc#471: a multi-byte load/store through a runtime pointer must seed
// FSR0 once and walk it with POSTINC0, not re-seed FSR0 from scratch for
// every byte.

unsigned long *volatile vp32;   // set by the test to point at buf32
unsigned long buf32;            // store destination

unsigned long *volatile vp32in; // set by the test to point at buf32in
unsigned long buf32in;          // load source, seeded by the test
unsigned long out32;            // load destination

void main(void) {
    unsigned long *p = vp32;
    *p = 0x12345678UL;

    unsigned long *q = vp32in;
    out32 = *q;
}
