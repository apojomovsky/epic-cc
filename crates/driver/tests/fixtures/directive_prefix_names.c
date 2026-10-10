/* Function labels that begin with assembler directive names (list, radix,
 * end). The listing emits them as bare labels; a prefix test must not read
 * them as directives or the functions drop out of the flash layout. */
volatile unsigned char out;

__attribute__((noinline)) void endpoint(void) { out = 1; }
__attribute__((noinline)) void listen(void) { out = out + 2; }
__attribute__((noinline)) void radix(void) { out = out + 4; }

void main(void) {
    endpoint();
    listen();
    radix();
}
