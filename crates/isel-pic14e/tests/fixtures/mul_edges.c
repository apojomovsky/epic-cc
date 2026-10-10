// Edge cases for the truncated shift-add multiply (__mul_u8 / __mul_u16).
// Inputs and outputs are volatile arrays so clang keeps every product as a
// runtime __mul call instead of folding it. Each index is one operand pair:
// zero operands (early exit), results that wrap mod 256 / mod 65536, and
// a long-multiplicand/short-multiplier split that exercises the u16 swap.
volatile unsigned char pa8[6];
volatile unsigned char pb8[6];
volatile unsigned char po8[6];
volatile unsigned int pa16[6];
volatile unsigned int pb16[6];
volatile unsigned int po16[6];

void main(void) {
    for (unsigned char i = 0; i < 6; i++) {
        po8[i] = (unsigned char)(pa8[i] * pb8[i]);
    }
    for (unsigned char i = 0; i < 6; i++) {
        po16[i] = (unsigned int)(pa16[i] * pb16[i]);
    }
}
