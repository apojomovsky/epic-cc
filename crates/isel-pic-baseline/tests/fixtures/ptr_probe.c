// P3 pointer/array probe: a runtime RAM pointer (FSR/INDF path) with a
// volatile index, no const (flash) involvement (that is P4). `volatile`
// everywhere keeps -O1 from folding the pointer away: the RAM GEP survives
// as a separate SSA value because the index comes from volatile input and
// the memory is volatile. `in` is a 16-bit volatile so clang keeps the
// index mask `& 3` as an i16 `and`. Expected: in = 1 -> ram[1] = 2 -> out
// = 2.
volatile unsigned short in;
volatile unsigned char out;
volatile unsigned char ram[8];
void main(void) {
    unsigned char i = (unsigned char)(in & 3);
    volatile unsigned char *p = ram + i;
    *p = (unsigned char)(i + 1);
    out = *p;
}
