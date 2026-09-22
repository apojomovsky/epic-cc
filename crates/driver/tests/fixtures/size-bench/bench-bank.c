volatile unsigned char in;
volatile unsigned char out;
unsigned char b0, b1, b2, b3, b4, b5, b6, b7;
void main(void) {
    b0 = in; b1 = b0; b2 = b1; b3 = b2;
    b4 = b3; b5 = b4; b6 = b5; b7 = b6;
    out = b7;
}
