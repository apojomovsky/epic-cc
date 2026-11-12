// The alloca sibling of gep_k_terms_ptr_value.c: a field address
// inside a padded local struct resolves to the frame base plus a
// constant offset (14) plus a scaled runtime index (12*j), and the
// pointer is materialized into a global pointer variable. A dropped
// constant offset reads the 0x62 sentinel at the element base, only
// the exact sum reads 0x63 (epic-cc#1461).
// Expected for in == 23 (j == 3): out == 0x63.
struct S { unsigned char a, b, c, d, e, f, g, h, i, j, k, l; };
struct Outer { unsigned char pad[8]; struct S arr[5]; };

volatile unsigned short in;
volatile unsigned char out;
volatile unsigned char *gp;

__attribute__((noinline)) static unsigned char read_gp(void) {
    return *gp;
}

void main(void) {
    struct Outer loc;
    unsigned char j = (unsigned char)(in & 3);
    loc.arr[3].g = 0x63;
    loc.arr[3].a = 0x62;
    gp = &loc.arr[j].g;
    out = read_gp();
}
