volatile unsigned char in;
volatile unsigned char out;
void main(void) {
    unsigned char v = in;
    unsigned char r = 0;
    switch (v) {
        case 0: r = 10; break;
        case 1: r = 20; break;
        case 2: r = 30; break;
        case 5: r = 50; break;
        case 10: r = 100; break;
        default: r = 99; break;
    }
    unsigned char r2 = 0;
    switch (v) {
        case 1: r2 = 5; // fallthrough
        case 2: r2 = (unsigned char)(r2 + 7); break;
        case 3: r2 = 30; break;
        default: r2 = 1; break;
    }
    out = (unsigned char)(r + r2);
}
