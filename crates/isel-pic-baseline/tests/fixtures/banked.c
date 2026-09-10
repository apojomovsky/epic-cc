// Milestone-2 bank acceptance: direct accesses to both banks (the D-2
// FSR<5> reassert), an i16 add, and indirect accesses through a runtime
// pointer to both banks (the per-touch FSR reload). Sized for the 509's
// two 16-byte GPR banks: buf[16] fills bank 0 (0x10-0x1F), the rest lands
// in bank 1 (0x30+).
//
// The indirect stores go through noinline functions so the pointer is
// genuinely runtime at the store: a store through a same-function GEP
// would fold to a direct access. Expected: buf[0] = 9, g_bank1 = 11,
// s3 = 3000, out = 2. Trace:
//   buf[0] = 5; g_bank1 = 7; out = 5 + 7 = 12 (direct both banks)
//   s1 = 1000; s2 = 2000; s3 = 3000 (i16 add); out = 0xB8 (i16 and-const)
//   set9(&buf[0]) writes 9 via FSR (indirect bank 0);
//   set11(&g_bank1) writes 11 via FSR (indirect bank 1)
//   buf[0] (9) > g_bank1 (11) is false, so the else arm runs: out = 2.
volatile unsigned char buf[16];
volatile unsigned char g_bank1;
volatile unsigned char out;
volatile unsigned short s1, s2, s3;
__attribute__((noinline)) void set9(volatile unsigned char *q) { *q = 9; }
__attribute__((noinline)) void set11(volatile unsigned char *q) { *q = 11; }
void main(void) {
    buf[0] = 5;
    g_bank1 = 7;
    out = buf[0] + g_bank1;
    s1 = 1000;
    s2 = 2000;
    s3 = s1 + s2;
    out = s3 & 0xFF;
    set9(&buf[0]);
    set11(&g_bank1);
    if (buf[0] > g_bank1)
        out = 1;
    else
        out = 2;
}
