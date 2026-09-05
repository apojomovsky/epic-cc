volatile unsigned char a;
volatile unsigned char b;
volatile unsigned char out;
void main(void) {
    a = 1;
    b = a + 2;
    out = a + b;
}
