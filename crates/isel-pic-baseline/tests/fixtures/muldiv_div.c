// P6 div-half acceptance: udiv/urem/sdiv/srem on i16 and i8 with a
// hand-computable `out`. Split from a combined fixture: baseline
// codegen runs ~18 words per IR instruction (D-2 selects on every
// access), so the combined main exceeded one 512-word page and could
// not link. Volatile discipline pins every op; urem uses 5 (div-rem
// reuse would fold `% 7`); b is runtime-made -19; i8 ops go through
// explicit casts plus a `gate` round-trip for genuine i8 SSA.
// Expected: in = 301 -> out = 21.
volatile unsigned int out;
volatile unsigned int in;
volatile unsigned char gate;

void main(void) {
    out = in / 7;                             // udiv i16: 43
    out = (in % 5) + out;                     // urem i16: 1 + 43 = 44
    int b = (int)in - 320;                    // -19 (runtime, so sdiv/srem survive)
    out = (unsigned int)(b / -3);             // sdiv i16: 6
    out = (unsigned int)(b % 3) + out;        // srem i16: -1 (0xFFFF) + 6 = 5
    gate = (unsigned char)in;                 // 45, barrier: genuine i8 SSA below
    unsigned char c = gate;
    out = out + (c % 4);                      // urem i8: 45 % 4 = 1 -> 6
    out = out + (c / 3);                      // udiv i8: 45 / 3 = 15 -> 21
}
