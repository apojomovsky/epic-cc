// Scaled GEP terms on the baseline core: a stride-3 record array and a
// 2x3 matrix indexed at runtime. The index accumulates in W (a MOVF then
// ADDWF chain on the term register) before FSR is set, so the test seeds
// every cell with a distinct value and a lost or extra repetition reads a
// neighbouring cell.
struct R { unsigned char a, b, c; };
volatile unsigned char in_i;
volatile unsigned char in_j;
volatile unsigned char out_r;
volatile unsigned char out_m;
volatile struct R tab[3];
volatile unsigned char m[2][3];
void main(void) {
    unsigned char i = in_i;
    unsigned char j = in_j;
    out_r = tab[i].b;
    out_m = m[i][j];
}
