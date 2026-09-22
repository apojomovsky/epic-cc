/* epic-cc#454: a mutable global with a non-zero initializer must actually be
 * initialized before main runs. Each global is read through a separate
 * function so constant folding cannot reduce it to a literal, and both are
 * `volatile` so they stay RAM globals.
 *
 * Hand-computed:  x = 0x5A, y = 0x1234
 *   f() reads x, g() reads y; main() stores their low bytes summed into the
 *   `out` global (a global, not the return value, so the assertion does not
 *   depend on either core's return-register convention).
 *   out = 0x5A + 0x34 = 0x8E
 */
static volatile unsigned char x = 0x5A;
static volatile unsigned int y = 0x1234;

volatile unsigned char out = 0;

unsigned char f(void) { return x; }
unsigned int g(void) { return y; }

int main(void) {
    out = (unsigned char)(f() + (unsigned char)g());
    return 0;
}
