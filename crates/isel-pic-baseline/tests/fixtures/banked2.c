volatile unsigned char a[8], b[8], c[8], d[8], e[8];
volatile unsigned char out;
volatile unsigned char idx;
void main(void) {
    idx = 3;
    a[idx] = 1;
    b[idx] = 2;
    c[idx] = 3;
    d[idx] = 4;
    e[idx] = 5;
    out = a[idx] + b[idx] + c[idx] + d[idx] + e[idx];
}
