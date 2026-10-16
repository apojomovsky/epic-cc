#define REG8(a) (*(volatile unsigned char *)(a))
volatile unsigned char out;
void main(void) {
    REG8(0x86) = 0x00;
    out = 0x01;
    out = 0x02;
}
