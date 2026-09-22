/* epic-cc#557: a zero-valued const scalar still needs its flash table byte.
 * Reading it must yield 0 on both cores. */
const unsigned char tab = 0;
volatile unsigned char out = 0;

int main(void) {
    out = tab;
    return 0;
}
