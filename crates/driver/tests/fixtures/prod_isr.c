// epic-cc#477 regression: main's in-flight PRODL/PRODH product must survive
// an ISR that multiplies. The __mul_* routines leave a partial product in
// PROD across instructions, so without the ISR saving PROD an interrupt
// lands mid-product and main resumes against the ISR's value.
volatile unsigned char in_a, in_b, out;
volatile unsigned char isr_in_a, isr_in_b, isr_out;

__attribute__((interrupt(0))) void isr(void) {
    isr_out = (unsigned char)(isr_in_a * isr_in_b);
}

void main(void) {
    out = (unsigned char)(in_a * in_b);
}
