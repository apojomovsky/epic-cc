// P6 i32 unsigned div acceptance: udiv/urem through the software
// routine copies, chained through one global (4 + 8 + 18 = 30 of 32
// bank bytes). Volatile pins every op against -O1.
// Expected: x = 0x12345678 -> 4.
volatile unsigned long x;

void main(void) {
    x = x + 0x11111111;               // setup: 0x23456789
    x = x / 7;                        // udiv i32: 84535864
    x = x % 5;                        // urem i32 of the quotient: 4
}
