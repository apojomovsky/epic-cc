// Fixture for #72: const array of a named struct with distinct non-zero
// field values. Exercises the named-struct global decoder (array and single)
// and the RETLW table readers on both PIC14 and PIC18.
typedef struct { unsigned char a; unsigned char b; unsigned char c; unsigned char d; } Desc;

const Desc tbl[2] = { { 11, 22, 33, 44 }, { 55, 66, 77, 88 } };
const Desc single = { 99, 101, 103, 105 };

volatile unsigned char idx;
volatile unsigned char out0, out1, out2, out3, out4, out5, out6, out7;
volatile unsigned char out_s0, out_s1, out_s2, out_s3;

void main(void) {
    // byte-wise flash reads through a cast, indices kept runtime via idx
    out0 = ((const unsigned char *)&tbl[0])[0];       // 11
    out1 = ((const unsigned char *)&tbl[0])[1];       // 22
    out2 = ((const unsigned char *)&tbl[1])[2];       // 77
    out3 = ((const unsigned char *)&tbl[idx])[3];     // idx-dependent
    // field reads (non-cast)
    out4 = tbl[0].a;                                  // 11
    out5 = tbl[1].b;                                  // 66
    out6 = tbl[idx].c;                                // idx-dependent
    out7 = tbl[idx].d;                                // idx-dependent
    // single named-struct global
    out_s0 = ((const unsigned char *)&single)[0];     // 99
    out_s1 = single.b;                                // 101
    out_s2 = ((const unsigned char *)&single)[2];     // 103
    out_s3 = single.d;                                // 105
}
