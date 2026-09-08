// Bit-fields acceptance (PIC14 parity, #268): a struct with bit-fields,
// written and read back through the same fields. Compiles through the whole
// driver pipeline and runs correctly in the simulator.
//
// in = 0x6D (01101101): f.a = in & 3 = 1, f.b = (in>>2) & 7 = 3,
// f.c = (in>>5) & 7 = 3. out = f.a | (f.b<<2) | (f.c<<5) = 1 | 12 | 96 = 109
// = 0x6D. The read-back path (f.a + f.b + f.c = 7) is exercised by the
// second store.
volatile unsigned char in;
volatile unsigned char out;
volatile unsigned char out2;
struct flags { unsigned char a:2; unsigned char b:3; unsigned char c:3; };
void main(void) {
    struct flags f;
    f.a = (unsigned char)(in & 3);
    f.b = (unsigned char)((in >> 2) & 7);
    f.c = (unsigned char)((in >> 5) & 7);
    out = (unsigned char)(f.a | (f.b << 2) | (f.c << 5));
    out2 = (unsigned char)(f.a + f.b + f.c);
}
