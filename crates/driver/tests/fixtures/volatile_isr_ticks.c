// ISR-shared multi-byte volatile counter (epic-cc#812 probe D).
// Both bytes of `ticks` must be re-read fresh at each snapshot point:
// no byte may be cached, reordered, or merged across the ISR boundary.
// The 0x1234 initializer rides the fixture's init store (past __start's
// zero-clear), so the run needs no poking.
volatile unsigned int ticks = 0x1234;
volatile unsigned char lo;
volatile unsigned char hi;

void __interrupt(0) isr(void) {
    ticks = (unsigned int)(ticks + 1u);
}

void main(void) {
    lo = (unsigned char)ticks;
    hi = (unsigned char)(ticks >> 8);
}
