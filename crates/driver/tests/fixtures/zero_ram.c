/* epic-cc#561: `buf` has no initializer, so __start clears it with the
 * zero-run loop instead of storing to it. The program reads it and forwards
 * the value. On real silicon the byte is indeterminate at power-on, and the
 * test seeds it non-zero to prove the clearing loop erased the seed. */
unsigned char buf[1];
volatile unsigned char out = 0;

int main(void) {
    out = buf[0];
    return 0;
}
