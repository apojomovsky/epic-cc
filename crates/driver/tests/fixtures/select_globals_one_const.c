// epic-cc#147: a pointer select with one const arm and one RAM arm must
// compile and read the selected arm's first byte.
//
// `ok_flag` is a runtime global, so clang cannot fold the select; the
// "PASS\n" literal is a const global and `ram_buf` is a RAM global, so the
// arms do not fold to a common base. The const arm must be copied to RAM
// (alloc) and the selected arm's address lands in the callee's pointer
// param slot.
//
// Expected (sim sets `ok_flag` before run):
//   - ok_flag = 1: out = 'P' (0x50)
//   - ok_flag = 0: out = 'R' (0x52, ram_buf[0])
volatile unsigned char ok_flag;
volatile unsigned char out;
volatile unsigned char ram_buf[4];

__attribute__((noinline)) static void putstr(const char *s)
{
    out = (unsigned char)s[0];
}

void main(void)
{
    // The compiler emits no RAM global initializers, so seed the buffer at
    // runtime; the select arm is still a RAM global.
    ram_buf[0] = 'R';
    putstr(ok_flag ? "PASS\n" : (const char *)ram_buf);
}
