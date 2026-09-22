struct S4 { unsigned char b[4]; };
struct S16 { unsigned char b[16]; };
volatile unsigned char sink;
struct S4 g4a, g4b;
struct S16 g16a, g16b;
void main(void) {
    g4b = g4a;
    g16b = g16a;
    sink = g4b.b[0] + g16b.b[0];
}
