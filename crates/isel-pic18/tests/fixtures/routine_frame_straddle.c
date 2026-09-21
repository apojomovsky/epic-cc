// Issue #509 acceptance: a PIC18 runtime routine's frame must stay inside
// ONE 256-byte BSR bank. The recipes test a frame byte with BTFSC/BTFSS and
// skip the instruction after it, which names a frame byte too; a MOVLB
// between the two changes the operand the skipped instruction reads.
//
// `big`'s 200-byte volatile pad pushes the overlay's derived base to 0xEE,
// so `__add_f32`'s 22-byte frame (a@0xEE, b@0xF2, __scr@0xF6..0x103) spans
// 0xEE..0x103 and crosses 0x100. alloc snaps it to the next bank start
// (0x100). Without that snap the recipe's `BTFSC <frame byte>,7` sits one
// byte above a `MOVLB 0x1`, the MOVLB lands inside the skip window, and the
// skipped operand is read from the wrong bank: the sim then reads 10.0f
// where 14.5f is due (3.0 + 1.5 + pad[7]=10.0).
//
// in = 3.0f -> out = 3.0 + 1.5 + 10.0 = 14.5f = 0x41680000.
volatile unsigned char in;
volatile float out;

__attribute__((noinline)) float big(float x) {
    volatile unsigned char pad[200];
    unsigned i;
    for (i = 0; i < 200; i++) pad[i] = (unsigned char)(i + in);
    return x + 1.5f + (float)pad[7];
}

void main(void) {
    out = big((float)in);
}
