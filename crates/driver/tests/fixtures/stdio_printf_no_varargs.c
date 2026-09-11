// epic-cc#391 second trigger: printf called with no extra args still
// leaves a zero-width va region pre-fix (same VaStart panic). The call
// passes only the format string, so no vararg is ever read.
#include <stdio.h>

volatile int g_n;

void putchar(char c) {
    (void)c;
    g_n++;
}

int main(void) {
    printf("hi\n");
    return 0;
}
