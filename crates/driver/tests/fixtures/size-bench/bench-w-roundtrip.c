/* Value parked in a slot and read straight back out of it (epic-cc#502):
 * the `Bin` result is stored via `MOVWF`, and the following store to `c`
 * re-read the same slot as a 2-word `MOVFF`. Sinking the result into the
 * second store removes the round trip. */
volatile unsigned char a;
volatile unsigned char b;
volatile unsigned char c;
void main(void) {
    c = (unsigned char)(a + b);
}
