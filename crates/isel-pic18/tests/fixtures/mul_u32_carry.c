// u32 hardware-multiply inter-group carry (epic-cc#1407): volatile seeds
// keep genuine `mul i32` shapes instead of folding (the -O1 IR carries a
// `mul i32` per pair, which legalize routes to the `__mul_u32` MULWF
// recipe). r0 is the reported failure (0xFF000001 before the fix); r2/r3
// stress stacked ADDWFC overflows; r1 is the old green vector; r4/r5/r6
// round out the byte lanes. No headers: the harness passes -nostdinc.
// Results hand-computed:
// r0 = FFFFFFFF * FFFFFFFF = 00000001
// r1 = 12345678 * 9ABCDEF1 = 366176F8
// r2 = FFFFFF00 * FFFFFF00 = 00010000
// r3 = 80000001 * 80000001 = 00000001
// r4 = FFFFFFFF * 00000002 = FFFFFFFE
// r5 = 00FF00FF * 00FF00FF = FC02FE01
// r6 = DEADBEEF * 00C0FFEE = 49F79332
volatile unsigned long a0 = 0xFFFFFFFFUL;
volatile unsigned long b0 = 0xFFFFFFFFUL;
volatile unsigned long a1 = 0x12345678UL;
volatile unsigned long b1 = 0x9ABCDEF1UL;
volatile unsigned long a2 = 0xFFFFFF00UL;
volatile unsigned long b2 = 0xFFFFFF00UL;
volatile unsigned long a3 = 0x80000001UL;
volatile unsigned long b3 = 0x80000001UL;
volatile unsigned long a4 = 0xFFFFFFFFUL;
volatile unsigned long b4 = 0x00000002UL;
volatile unsigned long a5 = 0x00FF00FFUL;
volatile unsigned long b5 = 0x00FF00FFUL;
volatile unsigned long a6 = 0xDEADBEEFUL;
volatile unsigned long b6 = 0x00C0FFEEUL;
volatile unsigned long r0, r1, r2, r3, r4, r5, r6;

void main(void) {
    r0 = a0 * b0;
    r1 = a1 * b1;
    r2 = a2 * b2;
    r3 = a3 * b3;
    r4 = a4 * b4;
    r5 = a5 * b5;
    r6 = a6 * b6;
}
