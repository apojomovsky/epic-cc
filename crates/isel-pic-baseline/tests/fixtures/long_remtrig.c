// P6 div borrow-fold regression, remainder-keep path: same loop as
// long_divtrig.c, observing rem instead of num.
// Expected: x = 40000000 -> 6576638 (0x6459FE).
volatile unsigned long x;

void main(void) {
    x = x % 0x00FF0001;
}
