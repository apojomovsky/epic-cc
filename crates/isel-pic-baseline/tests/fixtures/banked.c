// D-2 end-to-end acceptance (docs/37 section 3 P2): globals in both
// banks plus a genuinely runtime pointer (FSR/INDF) deref, exercising the
// `BCF`/`BSF FSR,5` reassertion sequence through real codegen. The 509's
// bank 0 GPR is 0x10-0x1F (16 bytes) and bank 1 is 0x30-0x3F (16 bytes),
// so 20 volatile i8 globals spill g16..g19 and `out` into bank 1. main
// writes 1..20 into g0..g19 (direct accesses to both banks), then reads
// g0 through a pointer loaded from a volatile pointer global (so it is
// genuinely runtime, not a folded static GEP) and stores it into out.
// Expected: out = 1.
volatile unsigned char g0;
volatile unsigned char g1;
volatile unsigned char g2;
volatile unsigned char g3;
volatile unsigned char g4;
volatile unsigned char g5;
volatile unsigned char g6;
volatile unsigned char g7;
volatile unsigned char g8;
volatile unsigned char g9;
volatile unsigned char g10;
volatile unsigned char g11;
volatile unsigned char g12;
volatile unsigned char g13;
volatile unsigned char g14;
volatile unsigned char g15;
volatile unsigned char g16;
volatile unsigned char g17;
volatile unsigned char g18;
volatile unsigned char g19;
volatile unsigned char out;
volatile unsigned char *volatile p;

unsigned char deref(unsigned char *q) {
    return *q;
}

void main(void) {
    unsigned char i;
    for (i = 0; i < 20; i++) {
        if (i == 0) g0 = 1;
        else if (i == 1) g1 = 2;
        else if (i == 2) g2 = 3;
        else if (i == 3) g3 = 4;
        else if (i == 4) g4 = 5;
        else if (i == 5) g5 = 6;
        else if (i == 6) g6 = 7;
        else if (i == 7) g7 = 8;
        else if (i == 8) g8 = 9;
        else if (i == 9) g9 = 10;
        else if (i == 10) g10 = 11;
        else if (i == 11) g11 = 12;
        else if (i == 12) g12 = 13;
        else if (i == 13) g13 = 14;
        else if (i == 14) g14 = 15;
        else if (i == 15) g15 = 16;
        else if (i == 16) g16 = 17;
        else if (i == 17) g17 = 18;
        else if (i == 18) g18 = 19;
        else g19 = 20;
    }
    p = &g0;
    out = deref(p);
}
