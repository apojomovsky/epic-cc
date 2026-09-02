// epic-cc#147 acceptance: a pointer select between two distinct global
// addresses must compile and read the selected global's first byte.
//
// `ok_flag` is a runtime global, so clang cannot fold the select; the two
// string literals are distinct const globals, so the select arms do not
// fold to a common base. The selected arm's address lands in the callee's
// pointer param slot and the first byte is read through it.
//
// Expected (sim sets `ok_flag` before run):
//   - ok_flag = 1: out = 'P' (0x50)
//   - ok_flag = 0: out = 'F' (0x46)
volatile unsigned char ok_flag;
volatile unsigned char out;

__attribute__((noinline)) static void putstr(const char *s)
{
    out = (unsigned char)s[0];
}

void main(void)
{
    putstr(ok_flag ? "PASS\n" : "FAIL\n");
}
