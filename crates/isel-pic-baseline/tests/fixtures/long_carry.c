// P6 mul carry-fold regression: iteration 1 folds t = 0xFF with
// carry set at byte 1, the wrap the INCFSZ skip chain must survive
// (epic-cc#328 review: pre-fix byte 2 reads 0x01, C starved).
// Expected: x = 0x0000FFC0 -> 0x0002FF40.
volatile unsigned long x;

void main(void) {
    x = x * 3;
}
