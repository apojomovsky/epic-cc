// Float conversion bank-select acceptance: the narrow-to-wide argument
// fills for __uitofp_f32/__sitofp_f32 must address the callee's `val`
// param slot through the bank select, not a hardcoded access-bank form.
// The 0x60-byte pad forces every frame (globals included) past the
// access window on the 4550, so the fill would read access-RAM garbage
// instead of the param bytes without the bank select.
// in = 3.0f: c = (u8)(4.0) = 4, k = (int)(5.0) = 5, u = (uint)(6.0) = 6,
// out = 4.0 + 5.0 + 6.0 = 15.0f = 0x41700000.
volatile unsigned char pad[0x60];
volatile float in;
volatile float out;

void main(void) {
    float a = in;                            // 3.0
    unsigned char c = (unsigned char)(a + 1.0f);
    int k = (int)(a + 2.0f);
    unsigned int u = (unsigned int)(a + 3.0f);
    out = (float)c + (float)k + (float)u;
}
