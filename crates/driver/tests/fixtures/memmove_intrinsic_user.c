volatile unsigned char in;
volatile unsigned char out;
volatile unsigned char buf[16];

void *memmove(void *dest, const void *src, unsigned short n) {
    unsigned char *d = (unsigned char *)dest;
    const unsigned char *s = (const unsigned char *)src;
    if (n == 0) return dest;
    if (d < s) {
        for (unsigned short i = 0; i < n; i++) d[i] = s[i];
    } else if (d > s) {
        unsigned short i = n;
        while (i > 0) { i--; d[i] = s[i]; }
    }
    return dest;
}

void main(void) {
    unsigned char n = (unsigned char)((in & 0x07) + 5);
    for (unsigned char i = 0; i < 16; i++) buf[i] = (unsigned char)(i * 7 + 3);
    __builtin_memmove((void *)&buf[2], (const void *)&buf[5], n);
    __builtin_memmove((void *)&buf[6], (const void *)&buf[4], n);
    unsigned char acc = 0;
    for (unsigned char i = 0; i < 16; i++) acc = (unsigned char)(acc + buf[i]);
    out = acc;
}
