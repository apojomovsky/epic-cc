// epic-cc#477 regression: main's in-flight PRODL/PRODH product must survive
// an ISR that multiplies. The __mul_* routines leave a partial product in
// PROD across instructions, so without the ISR saving PROD an interrupt
// lands mid-product and main resumes against the ISR's value.
//
// The inputs carry real initializers: __start clears zero-initialized
// globals before main (epic-cc#561), which would erase a sim-side seed.
volatile unsigned char in_a = 47, in_b = 5, out;
volatile unsigned char isr_in_a = 3, isr_in_b = 7, isr_out;

__attribute__((interrupt(0))) void isr(void) {
    isr_out = (unsigned char)(isr_in_a * isr_in_b);
}

void main(void) {
    out = (unsigned char)(in_a * in_b);
}
