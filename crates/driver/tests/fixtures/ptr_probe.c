// Milestone-5 pointer/const probe: a runtime RAM pointer (FSR/INDF path) and
// a const-table read (RETLW path) in one program (docs/11). `volatile`
// everywhere keeps -O1 from folding the pointer away: the RAM GEP survives as
// a separate SSA value because the index comes from volatile input and the
// memory is volatile. `in` is a 16-bit volatile so clang keeps the index mask
// `& 3` as an i16 `and` (isel lowers i16 and; it has no i8 and). Expected:
// in = 1 -> ram[1] = table[1] = 20 -> out = 20.
volatile unsigned short in;
volatile unsigned char out;
static const unsigned char table[4] = {10, 20, 30, 40};
volatile unsigned char ram[8];
void main(void) {
    unsigned char i = (unsigned char)(in & 3);
    volatile unsigned char *p = ram + i;
    *p = table[i];
    out = *p;
}
