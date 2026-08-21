#include <epic-cc.h>
EPIC_NAKED void my_mul(void) {
    asm("movf 0x20, w");
    asm("addwf 0x21, w");
    asm("movwf 0x22");
    asm("return");
}
volatile unsigned char a, b, r;
void main(void) { a=3; b=4; my_mul(); }
