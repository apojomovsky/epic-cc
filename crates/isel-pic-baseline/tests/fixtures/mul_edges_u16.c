// Baseline __mul_u16 edge case: 65535 * 2 wraps mod 65536 and drives the operand swap.
volatile unsigned int pa16;
volatile unsigned int pb16;

void main(void) {
    pa16 = (unsigned int)(pa16 * pb16);
}
