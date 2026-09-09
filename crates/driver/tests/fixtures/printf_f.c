#include <stdio.h>
volatile char g_buf[16];
volatile int g_n = 0;
void putchar(char c) {
    g_buf[g_n] = c;
    g_n++;
}
void main(void) {
    double d = 3.5;
    printf("d=%f\r\n", d);
    for (;;) {}
}
