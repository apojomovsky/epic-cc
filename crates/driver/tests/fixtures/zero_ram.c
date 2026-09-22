/* epic-cc#557: `buf` has no initializer, so it is never written by __start.
 * The program reads it and forwards the value. On a simulator that seeds RAM
 * with zero this reads 0; on real silicon the byte is indeterminate, and
 * seeding it non-zero in the test shows the value survives untouched. */
unsigned char buf[1];
volatile unsigned char out = 0;

int main(void) {
    out = buf[0];
    return 0;
}
