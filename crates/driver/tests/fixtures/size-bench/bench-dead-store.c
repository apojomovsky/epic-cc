volatile unsigned char in;
volatile unsigned char out;
unsigned char slot;
void main(void) {
    slot = in;
    out = slot;
}
