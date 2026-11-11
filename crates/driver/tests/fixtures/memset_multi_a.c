// Multi-unit memset intrinsic shape (epic-cc#1448): this unit and
// memset_multi_b.c each emit llvm.memset, and neither includes
// <string.h>. One linked memset implementation must serve both.
// big_a.b[1] is 9 before zeroing, so out == 0 + 2 == 2 only when
// both units' zero-initializations really run.
struct BigA { unsigned char b[16]; };

static struct BigA big_a;
volatile unsigned char out;

extern volatile unsigned char out_b;
void touch_b(unsigned char v);

void main(void) {
    big_a.b[1] = 9;
    big_a = (struct BigA){0};
    touch_b(2);
    out = (unsigned char)(big_a.b[1] + out_b);
}
