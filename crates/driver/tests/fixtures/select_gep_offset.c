// epic-cc#781: a pointer select with a nonzero-offset GEP arm across
// distinct bases must compile and read the selected arm's byte.
//
// `ok_flag` is a runtime global, so clang cannot fold the select; one arm
// is `"ab" + 1` (a one-hop GEP over a const) and the other is a RAM
// global, so the arms share no base and cannot fold. iselcore seeds the
// dst as an indirect slot and each backend materializes base plus k.
//
// Expected:
//   - ok_flag = 1: out = 'b' (0x62, "ab"[1])
//   - ok_flag = 0: out = 'R' (0x52, ram_buf[0])
#ifndef OK_FLAG
#define OK_FLAG 1
#endif
volatile unsigned char ok_flag = OK_FLAG; /* tests compile with -D OK_FLAG=n */
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
    putstr(ok_flag ? "ab" + 1 : (const char *)ram_buf);
}
