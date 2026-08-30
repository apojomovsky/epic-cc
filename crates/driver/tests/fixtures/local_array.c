// Local-array alloca probe for #149: a stack [N x i8] buffer written
// and read at a runtime index, the local counterpart to the global
// array.c. Acceptance: local buffer survives GEP and load/store lowering
// and runs correctly on the simulator.
//
// Uses volatile globals for the index and result so clang cannot fold the
// access. The conversion-buffer pattern from epic-serial (char buf[5])
// is reduced to buf[8] here to keep the index mask `& 7` as i16 and
// exercise the FSR/INDF path.
volatile unsigned short in;
volatile unsigned char out;

void main(void) {
    // Local buffer, the shape that previously hit SPIKE: unsupported type "[8"
    char buf[8];
    unsigned char i = (unsigned char)(in & 7);
    buf[i] = (unsigned char)(i + 1);
    out = (unsigned char)buf[i];
}
