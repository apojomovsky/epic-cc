// P6 u16 div borrow-fold regression through the __scr den copy:
// den = 0xFF02 wraps the byte-1 fold with borrow pending, and the
// param slot may sit in another bank than rem (epic-cc#328 review).
// Pre-fix this yields 0x8080.
// Expected: x = 65535, y = 65282 -> 1.
volatile unsigned int x;
volatile unsigned int y;

void main(void) {
    x = x / y;
}
