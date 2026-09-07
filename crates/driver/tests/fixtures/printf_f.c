#include <stdio.h>
volatile char g_buf[16];
volatile int g_n = 0;
int putchar(int c) {
    g_buf[g_n] = (char)c;
    g_n++;
    return c;
}
void main(void) {
    double d = 3.5;
    printf("d=%f\r\n", d);
    for (;;) {}
}
