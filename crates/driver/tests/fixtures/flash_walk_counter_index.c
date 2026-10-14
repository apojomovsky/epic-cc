#include <stdint.h>

/* A counter-indexed load through an advancing walk phi over a flash
 * string too long for a RAM copy: the slot holds a flash address that
 * INDF would read as RAM, so the PIC14 family refuses it (epic-cc#1197,
 * epic-cc#1203). The counter-plus-gep shape is also what the NUL-walk
 * planner looks for; it must stay out for the same reason. */
#define T10 "0123456789"
#define T100 T10 T10 T10 T10 T10 T10 T10 T10 T10 T10

static const char big[] = T100 T100 T100 "end";

volatile uint8_t n;

int main(void)
{
    const char *p = big;
    uint8_t i = 0;
    while (p[i] != 0) {
        p = p + 1;
        i++;
    }
    n = i;
    return 0;
}
