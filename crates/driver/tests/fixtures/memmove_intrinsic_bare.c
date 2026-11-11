volatile unsigned char in;
volatile unsigned char out;
volatile unsigned char buf[16];

void main(void) {
    unsigned char n = (unsigned char)((in & 0x07) + 5);
    for (unsigned char i = 0; i < 16; i++) buf[i] = (unsigned char)(i * 7 + 3);
    __builtin_memmove((void *)&buf[6], (const void *)&buf[4], n);
    __builtin_memmove((void *)&buf[1], (const void *)&buf[3], n);
    unsigned char acc = 0;
    for (unsigned char i = 0; i < 16; i++) acc = (unsigned char)(acc + buf[i]);
    out = acc;
}
