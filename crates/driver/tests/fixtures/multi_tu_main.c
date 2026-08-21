// Multi-TU acceptance: `total` is defined here and written by both units;
// `bump` and `scratch` exist in BOTH a.c and b.c as statics, so llvm-link
// must rename one of each pair. Result is computed by hand below.
unsigned char total;
extern unsigned char from_a(unsigned char);
extern unsigned char from_b(unsigned char);

void main(void) {
    total = from_a(3) + from_b(4);   // 4 + 6 = 10 (0x0A)
}
