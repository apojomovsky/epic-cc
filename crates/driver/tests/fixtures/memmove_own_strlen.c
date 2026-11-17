// A bare __builtin_memmove program that defines its own strlen
// (epic-cc#1451): supplying the memmove body must not drag in a
// second strlen definition. The user's strlen always returns 0xEE,
// so out == (buffer checksum after the moves) + 0xEE proves both
// bodies are the ones running. No <string.h> is included.
volatile unsigned char in;
volatile unsigned char out;
volatile unsigned char buf[16];

unsigned int strlen(const char *s) {
    (void)s;
    return 0xEE;
}

void main(void) {
    unsigned char n = (unsigned char)((in & 0x07) + 5);
    for (unsigned char i = 0; i < 16; i++) buf[i] = (unsigned char)(i * 7 + 3);
    __builtin_memmove((void *)&buf[6], (const void *)&buf[4], n);
    __builtin_memmove((void *)&buf[1], (const void *)&buf[3], n);
    unsigned char acc = 0;
    for (unsigned char i = 0; i < 16; i++) acc = (unsigned char)(acc + buf[i]);
    out = (unsigned char)(acc + (unsigned char)strlen((const char *)buf));
}
