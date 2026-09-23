// epic-cc#471: a multi-byte load/store through a runtime pointer must seed
// FSR0 once and walk it with POSTINC0, not re-seed FSR0 from scratch for
// every byte.

unsigned long buf32;                       // store destination
unsigned long *volatile vp32 = &buf32;     // points at buf32

unsigned long buf32in = 0x12345678UL;      // load source, carried by the initializer
unsigned long *volatile vp32in = &buf32in; // points at buf32in
unsigned long out32;                       // load destination

void main(void) {
    unsigned long *p = vp32;
    *p = 0x12345678UL;

    unsigned long *q = vp32in;
    out32 = *q;
}
