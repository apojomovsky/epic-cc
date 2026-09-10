// P6 mul-half acceptance: mul plus const and variable-count shifts on
// i16 and i8 with a hand-computable `out`. Split from a combined
// fixture for the same page reason as muldiv_div.c: ~18 words per IR
// instruction caps mains well under 512 words. `in` reloads keep
// main's frame small (routine params must still fit bank 0).
// Expected: in = 301 -> out = 616.
volatile unsigned int out;
volatile unsigned int in;
volatile unsigned char gate;

void main(void) {
    out = in * 3;                             // mul i16: 903
    out = out << 2;                           // shl i16 const: 3612
    out = (out >> 3) | (in >> 4);             // lshr i16 const: 451 | 18 = 467
    gate = (unsigned char)in;                 // 45, barrier: genuine i8 SSA below
    unsigned char c = gate;
    unsigned char w = (unsigned char)(c * 7);   // mul i8: (45*7)&0xFF = 59
    out = out + w;                            // 467 + 59 = 526
    out = out + ((unsigned int)c << (unsigned char)(in & 3));  // shl var i16: 526 + 45<<1 = 616
}
