/* Interrupt-cost fixture for PIC18 (epic-cc#840). Same shape as the
 * PIC14 twin: the ISR flags its entry and counts the tick, main spins
 * in a volatile window while the ladder injects the interrupt. Uses
 * the interrupt attribute spelling the PIC18 backend lowers. */
volatile unsigned char bench_sync;
volatile unsigned char bench_isr;
volatile unsigned char ticks;
volatile unsigned char spin = 200;

__attribute__((interrupt(0))) void isr(void) {
    bench_isr = 1;
    ticks++;
}

void main(void) {
    bench_sync = 1;
    while (spin-- > 0) {
    }
    bench_sync = 2;
}
