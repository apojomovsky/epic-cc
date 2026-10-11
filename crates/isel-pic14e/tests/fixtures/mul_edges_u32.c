// Edge cases for the early-exit __mul_u32 multiply. Inputs and outputs are
// volatile arrays so clang keeps every product as a runtime __mul call.
// Each index is one operand pair: a zero multiplier (early exit), a
// short-a / long-b pair that takes the operand swap, a product that wraps
// mod 2^32, and a full-width pair that runs every pass. The product is
// written back into pa32 so the RAM pool holds only two arrays.
volatile unsigned long pa32[4];
volatile unsigned long pb32[4];

void main(void) {
    for (unsigned char i = 0; i < 4; i++) {
        pa32[i] = (unsigned long)(pa32[i] * pb32[i]);
    }
}
