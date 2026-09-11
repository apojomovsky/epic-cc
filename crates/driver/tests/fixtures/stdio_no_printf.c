// epic-cc#391 regression: including <stdio.h> without ever calling
// printf must compile. The driver still injects the stdio runtime (so a
// putchar sink is required), but its variadic printf has no call sites;
// its va_start needs a region even so.
#include <stdio.h>

void putchar(char c) {
    (void)c;
}

int main(void) {
    return 0;
}
