// Main and the ISR both materialize a scaled GEP pointer value
// (items + 6 + 12*idx) into a global pointer. The test fires the
// interrupt after main's low seed byte lands in the shared park
// pair; only an ISR save/restore of that pair keeps main's sum
// exact. Sentinels are planted through the call-argument path.
// The arrays are declared small for the banked GPR windows; the
// indexes probe the flat RAM address space past them.
// Expected: out == 0x51 (main), out2 == 0x52 (ISR).
struct S { unsigned char a, b, c, d, e, f, g, h, i, j, k, l; };

volatile unsigned short in;
volatile unsigned short in2;
volatile unsigned char out;
volatile unsigned char out2;
volatile struct S items[8];
volatile struct S items2[8];
volatile unsigned char *gp;
volatile unsigned char *gp2;

__attribute__((noinline)) static void putb(unsigned char *p, unsigned char v) {
    *p = v;
}

__attribute__((noinline)) static unsigned char read_gp(void) {
    return *gp;
}

__attribute__((interrupt(0))) void isr(void) {
    gp2 = &items2[in2].g;
    out2 = *gp2;
}

void main(void) {
    unsigned short idx = in;
    putb(&items[idx].g, 0x51);
    putb(&items[idx].a, 0x50);
    putb(&items2[in2].g, 0x52);
    gp = &items[idx].g;
    out = read_gp();
}
