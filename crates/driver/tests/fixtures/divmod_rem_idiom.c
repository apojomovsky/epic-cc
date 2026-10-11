/* Rem idiom coverage (epic-cc#1008). Operands are non-volatile loop values,
 * so clang -O1 spells each `%` as `x - (x / y) * y`, the shape the divmod
 * fusion recovers. The reference is repeated subtraction: clang cannot fold
 * it into the identity that would erase the division, so the idiom survives
 * to legalize. Plain C99. */
#include <stdint.h>

volatile unsigned char fails;

void main(void) {
    uint16_t i;
    for (i = 0; i < 400; i++) {
        uint16_t a = (uint16_t)((i * 37u + 11u) % 600u);
        uint16_t b = (uint16_t)(i % 97u + 1u);
        uint16_t q = a / b;
        uint16_t m = a % b;
        uint16_t rq = 0;
        uint16_t rr = a;
        while (rr >= b) {
            rr = (uint16_t)(rr - b);
            rq++;
        }
        if (q != rq) fails++;
        if (m != rr) fails++;
    }
    for (i = 0; i < 400; i++) {
        int16_t a = (int16_t)((i * 37u) % 600u) - 300;
        int16_t b = (int16_t)(i % 13u) - 6;
        int16_t q, m, ua, ub, uq, ur;
        if (b == 0) continue;
        q = a / b;
        m = a % b;
        ua = a < 0 ? (int16_t)-a : a;
        ub = b < 0 ? (int16_t)-b : b;
        uq = 0;
        ur = ua;
        while (ur >= ub) {
            ur = (int16_t)(ur - ub);
            uq++;
        }
        if ((a < 0) != (b < 0)) uq = (int16_t)-uq;
        if (a < 0) ur = (int16_t)-ur;
        if (q != uq) fails++;
        if (m != ur) fails++;
    }
}
