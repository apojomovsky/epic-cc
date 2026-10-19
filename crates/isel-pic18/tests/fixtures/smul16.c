// Signed 16x16 -> 32 widening products (epic-cc#1344): volatile seeds keep
// clang's sext + mul i32 shape instead of folding (the -O1 IR carries two
// `sext i16 -> i32` into each `mul i32`, which legalize narrows to the
// `__mul_s16` MULWF recipe). One pair per sign combination, results
// hand-computed:
// r0 = 256 * 40 = 10240 = 0x2800
// r1 = -40 * -64 = 2560 = 0x0A00
// r2 = -32768 * 32767 = -1073709056 = 0xC0008000
// r3 = 12345 * -6789 = -83810205 = 0xFB012863
volatile int a0 = 256;
volatile int b0 = 40;
volatile int a1 = -40;
volatile int b1 = -64;
volatile int a2 = -32768;
volatile int b2 = 32767;
volatile int a3 = 12345;
volatile int b3 = -6789;
volatile long r0, r1, r2, r3;

void main(void) {
    r0 = (long)a0 * (long)b0;
    r1 = (long)a1 * (long)b1;
    r2 = (long)a2 * (long)b2;
    r3 = (long)a3 * (long)b3;
}
