// Second unit of the multi-unit memset shape (epic-cc#1448): zeroes
// its struct, refills the first three bytes with v, and exports the
// result. For v == 2: out_b == big_b.b[0] + big_b.b[5] == 2 + 0 == 2.
struct BigB { unsigned char b[16]; };

static struct BigB big_b;
volatile unsigned char out_b;

void touch_b(unsigned char v) {
    big_b.b[0] = 7;
    big_b = (struct BigB){0};
    __builtin_memset(big_b.b, v, 3);
    out_b = (unsigned char)(big_b.b[0] + big_b.b[5]);
}
