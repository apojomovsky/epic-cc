// P6 div borrow-fold regression: den is 01 00 FF 00, so the byte-2
// trial fold wraps with borrow pending in early iterations (same
// INCFSZ skip-chain class as long_carry.c, epic-cc#328 review).
// Expected: x = 40000000 -> 2.
volatile unsigned long x;

void main(void) {
    x = x / 0x00FF0001;
}
