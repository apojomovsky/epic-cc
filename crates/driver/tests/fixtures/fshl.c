// fshl/fshr probe for #145: epic-math LFSR rotate idiom.
volatile unsigned short in;
volatile unsigned short out;
// Repro from #145: step expands to llvm.fshl.i16 via (s>>1)|(bit<<15)
unsigned short step(unsigned short s) {
    unsigned short bit = (unsigned short)(((s >> 0) ^ (s >> 2) ^ (s >> 3) ^ (s >> 5)) & 1u);
    return (unsigned short)((s >> 1) | (bit << 15));
}
void main(void) {
    // in=0xACE1 is a known LFSR state; step should produce deterministic value
    // For verification the simulator checks out == step(in)
    out = step(in);
}
