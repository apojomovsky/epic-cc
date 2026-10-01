/* Optimization-profile matrix fixture (epic-cc#839). One small program
 * that exercises every per-profile pipeline difference (constant
 * folding, LSR over the index walk, both inline tiers through `work`
 * and `step`, PIC18 code factoring over the repeated tail), with a
 * checksum the sim test pins across all four profiles. */
volatile unsigned char in[8] = {3, 1, 4, 1, 5, 9, 2, 6};
volatile unsigned char checksum;

static unsigned char step(unsigned char x) {
    return (unsigned char)(x * 7 + 3);
}

static unsigned char work(unsigned char x) {
    return step((unsigned char)(x ^ 0x5A));
}

void main(void) {
    unsigned char acc = 0;
    unsigned char i;
    for (i = 0; i < 8; i++) {
        acc += work(in[i]);
    }
    acc += step(0x11);
    acc += step(0x22);
    acc += step(0x33);
    checksum = acc;
}
