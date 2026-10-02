/* Interrupt-cost fixture for the 14-bit core (epic-cc#840). The ISR
 * does the smallest observable work: flag the entry, count the tick.
 * The ladder fires the interrupt while main spins in a volatile
 * window (bench_sync == 1, read off --map like bench_mark) and
 * records vector-to-first-store latency plus the full round trip.
 * Uses the __interrupt spelling the PIC14 backend lowers. */
volatile unsigned char bench_sync;
volatile unsigned char bench_isr;
volatile unsigned char ticks;
volatile unsigned char spin = 200;

void __interrupt(0) isr(void) {
    bench_isr = 1;
    ticks++;
}

void main(void) {
    bench_sync = 1;
    while (spin-- > 0) {
    }
    bench_sync = 2;
}
