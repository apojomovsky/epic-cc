// epic-cc#783 regression: main's in-flight PRODL/PRODH product must survive
// an ISR that never multiplies. The narrowed save set drops PROD entirely,
// so the interrupt lands mid-product with no PROD save and main still
// resumes against its own value. The twin of prod_isr.c, whose ISR does
// multiply and must keep the PROD save.
//
// The inputs carry real initializers: __start clears zero-initialized
// globals before main (epic-cc#561), which would erase a sim-side seed.
volatile unsigned char in_a = 47, in_b = 5, out;
volatile unsigned char isr_in_a = 3, isr_in_b = 7, isr_out;

__attribute__((interrupt(0))) void isr(void) {
    isr_out = (unsigned char)(isr_in_a + isr_in_b);
}

void main(void) {
    out = (unsigned char)(in_a * in_b);
}
