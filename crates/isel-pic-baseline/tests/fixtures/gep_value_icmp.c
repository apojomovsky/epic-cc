// A pointer end computed from a loaded base plus a constant offset
// and a runtime term (`base + 2 + (in - 4)`) is a GEP value with no
// slot of its own: the allocator folds it into its uses. Comparing a
// walking cursor against that end (icmp eq/ne over the pointer) must
// materialize the address, not look the GEP up as a slot
// (epic-cc#1472). With in == 6 the end is arr + 4 and the cursor
// starts at arr + 2, so the loop body runs 2 times and the cursor
// lands exactly on the end: out == 2, out2 == 1. The array stays
// small because the baseline device has only two GPR banks.
volatile unsigned char arr[16];
volatile unsigned short in;
volatile unsigned char out;
volatile unsigned char out2;

struct H {
    unsigned char a, b;
    unsigned char *p;
};
volatile struct H h;

void main(void)
{
    unsigned char *base;
    unsigned char *end;
    unsigned char *cur;
    unsigned char n = 0;

    h.p = arr;
    base = h.p;
    end = base + 2 + (in - 4);
    cur = base + 2;
    while (cur != end) {
        n++;
        cur++;
    }
    out = n;
    out2 = (cur == end);
}
