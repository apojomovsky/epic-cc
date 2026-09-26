void _delay(unsigned long cycles);

typedef void (*cb_t)(unsigned long);

volatile unsigned char seen;

static volatile cb_t handler;

static void g1(unsigned long n) {
    seen = (unsigned char)n;
}

void main(void) {
    handler = g1;
    handler(10);
    _delay(100);
}
