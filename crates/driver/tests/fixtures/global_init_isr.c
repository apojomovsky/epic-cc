/* epic-cc#454, ISR variant: the PIC14/PIC14E backends emit `__start` after
 * the ISR body (pass B), and that separate emitter used to write placeholder
 * zeros for pointer-initializer bytes, so `p` came out NULL whenever an ISR
 * existed. Both emitters now materialize the ref.
 *
 * Hand-computed: g = 0x5A at a RAM address, p = &g. `out` receives the value
 * of *p, which is 0x5A only if p was initialized to g's address (a NULL p
 * would fault or read address 0).
 */
volatile unsigned char g = 0x5A;
volatile unsigned char *volatile p = &g;

volatile unsigned char out = 0;

__attribute__((interrupt(0))) void isr(void) { }

int main(void) {
    out = *p;
    return 0;
}
