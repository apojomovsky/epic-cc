// Edge cases for the early-exit __mul_u32 multiply. Inputs and outputs are
// volatile arrays so clang keeps every product as a runtime __mul call.
// Each index is one operand pair: a zero multiplier (early exit), a
// short-a / long-b pair, and a full-width pair that wraps mod 2^32. The
// product is written back into pa32 so each array fits one 16F505 bank.
volatile unsigned long pa32[3];
volatile unsigned long pb32[3];

void main(void) {
    for (unsigned char i = 0; i < 3; i++) {
        pa32[i] = (unsigned long)(pa32[i] * pb32[i]);
    }
}
