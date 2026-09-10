// P6 i32 signed div acceptance: sdiv/srem through the software
// routine copies, seeded negative via RAM bytes (volatile pins the
// ops against -O1, no runtime-made local needed). 4 + 8 + 20 = 32
// of 32 bank bytes: exactly full, nothing more fits here.
// Expected: x = -123456789 -> -4 (0xFFFFFFFC).
volatile long x;

void main(void) {
    x = x / 7;                        // sdiv i32: -17636684
    x = x % -5;                       // srem i32, negative divisor: -4
}
