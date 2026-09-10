#include <malloc.h>
unsigned char heap[64];
volatile unsigned char out;
void main(void) {
    unsigned char _MALLOC_SPEC *a;
    unsigned char _MALLOC_SPEC *b;
    _initHeap(heap, sizeof heap);
    a = malloc(4);
    b = malloc(4);
    a[0] = 0x11;
    a[3] = 0x22;
    b[0] = 0x33;
    b[3] = 0x44;
    free(a);
    {
        unsigned char _MALLOC_SPEC *c;
        c = malloc(3);
        if (b == 0 || c == 0) {
            out = 0x00;
        } else {
            c[0] = 0x55;
            c[2] = 0x66;
            out = (unsigned char)(b[0] ^ b[3] ^ c[0] ^ c[2] ^ 0x7F);
        }
    }
    __asm__("sleep");
}
