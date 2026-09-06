// Milestone-6 scalar acceptance: a loop over a hand-computable computation
// that exercises the full scalar surface — `sub`, `and i8`, `or`, `xor`,
// and the `eq`/`ne`/`ugt`/`ult` comparison predicates — with no `mul`/
// `shl`/`div` (clang folds `i + i` into `shl`, so the doubling is written
// as plain adds of the loop counter). Everything is i8/i16 scalar: no
// pointers, no structs, no arrays.
//
// Expected: in = 7 -> n = 7 -> out = 174. Trace (8-bit wraps, `^` before
// `|`/`+` in the else branch is `(s - i) ^ 0x55`):
//   i=0 even: s = 0+0 = 0; i>2 no; i!=4 yes -> 1; s>200 no            -> 1
//   i=1 odd : s = (1-1)^0x55 = 0x55; i==1 -> |0x80 = 0xD5; s<10 no     -> 213
//   i=2 even: s = 213+2 = 215; i>2 no; i!=4 yes -> 216; 216^0x55=0x8D  -> 141
//   i=3 odd : s = (141-3)^0x55 = 0xDF; i==1 no; s<10 no                -> 223
//   i=4 even: s = 223+4 = 227; i>2 -> |0x10 = 0xF3; i!=4 no; 243^0x55  -> 166
//   i=5 odd : s = (166-5)^0x55 = 0xF4; i==1 no; s<10 no                -> 244
//   i=6 even: s = 244+6 = 250; i>2 -> |0x10 (no change); i!=4 -> 251;
//             251^0x55 = 0xAE                                            -> 174
volatile unsigned char in;
volatile unsigned char out;
void main(void) {
    unsigned char n = in & 0x07;
    unsigned char s = 0;
    unsigned char i;
    for (i = 0; i < n; i++) {
        if ((i & 1) == 0) {
            s = (unsigned char)(s + i);
            if (i > 2) s = (unsigned char)(s | 0x10);
            if (i != 4) s = (unsigned char)(s + 1);
            if (s > 200) s = (unsigned char)(s ^ 0x55);
        } else {
            s = (unsigned char)((s - i) ^ 0x55);
            if (i == 1) s = (unsigned char)(s | 0x80);
            if (s < 10) s = (unsigned char)(s + 3);
        }
    }
    out = s;
}
