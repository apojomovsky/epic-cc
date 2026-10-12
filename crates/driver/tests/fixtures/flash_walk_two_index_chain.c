#include <stdint.h>
/* Two runtime indices over one flash table whose row stride (8) makes the
 * TBLPTR shift-add chain win (epic-cc#1126). `TABLE[a][b]` resolves to the
 * terms (8, a) and (1, b): the chain folds `a` in, the skip path adds `b`.
 * With `a = 2, b = 3` the read lands on flat byte 19. */
static const uint8_t TABLE[4][8] = {
    { 0, 1, 2, 3, 4, 5, 6, 7 },
    { 8, 9, 10, 11, 12, 13, 14, 15 },
    { 16, 17, 18, 19, 20, 21, 22, 23 },
    { 24, 25, 26, 27, 28, 29, 30, 31 },
};
volatile uint8_t a;
volatile uint8_t b;
volatile uint8_t out;
int main(void) {
    a = 2;
    b = 3;
    out = TABLE[a][b];
    return 0;
}
