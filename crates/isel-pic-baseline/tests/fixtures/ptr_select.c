// Pointer select between two offsets of one array: the selected arm's
// address must be materialized, so reading through the result yields
// the selected arm's byte. buf[1] and buf[2] hold distinct bytes.
volatile unsigned char in;
volatile unsigned char out;
volatile unsigned char buf[4];
void main(void) {
    volatile unsigned char *p = in ? buf + 2 : buf + 1;
    out = *p;
}
