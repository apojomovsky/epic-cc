// Issue #5: const (flash) structs. Exercises the flat-byte decode of
// clang's literal struct initializers, array-of-struct element GEPs
// (variable index), byval copies of const structs (plain global and
// inlined-GEP element forms), and runtime byte-indexed flash reads of a
// const struct (the RETLW-table path). All reads land in volatile
// globals read back by the sim.
//
// NOTE: no constant-length `__builtin_memcpy(buf, &C1, 4)` — clang -O1
// folds a memcpy from a known const into a constant store, so no flash
// read reaches the IR. The `(const unsigned char *)&C1` byte reads keep
// the index runtime (`idx`) so the RETLW readers actually run.
struct Pair { char a; short b; };

const struct Pair C1 = { 'A', 0x1234 };
const struct Pair CARR[2] = { { 'D', 0x1111 }, { 'E', 0x2222 } };

volatile unsigned char idx;
volatile unsigned char out_a, out_a2, out_a3, out_m0, out_m1;
volatile unsigned short out_b, out_b2, out_b3;

__attribute__((noinline)) void byval_c1(struct Pair p) {
    out_a = p.a;
    out_b = p.b;
}
__attribute__((noinline)) void byval_elem1(struct Pair p) {
    out_a2 = p.a;
    out_b2 = p.b;
}
__attribute__((noinline)) void byval_var(struct Pair p) {
    out_a3 = p.a;
    out_b3 = p.b;
}

void main(void) {
    byval_c1(C1);            // byval of a const struct (plain global)
    byval_elem1(CARR[1]);    // byval of a const struct element (inlined GEP)
    byval_var(CARR[idx]);    // byval with a variable element index
    out_m0 = ((const unsigned char *)&C1)[idx];      // flash byte read
    out_m1 = ((const unsigned char *)&C1)[idx + 2];  // flash byte read, +2
}