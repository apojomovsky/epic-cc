// Overlay acceptance: two sibling functions big_a/big_b each carry >= 16
// bytes of simultaneous live i16 locals (8 x int, kept live by volatile
// stores to `sink` so -O1 cannot fold them away), called sequentially from
// main. The allocator must overlay their frames (never co-live) so total
// bank-0 demand stays below the sum of the three functions' demands.
volatile unsigned char in;
volatile unsigned char out;
volatile int sink;

__attribute__((noinline)) static int big_a(int x) {
    int t0 = x + 0, t1 = x + 1, t2 = x + 2, t3 = x + 3;
    int t4 = x + 4, t5 = x + 5, t6 = x + 6, t7 = x + 7;
    sink = t0; sink = t1; sink = t2; sink = t3;
    sink = t4; sink = t5; sink = t6; sink = t7;
    return t0 + t1 + t2 + t3 + t4 + t5 + t6 + t7;
}
__attribute__((noinline)) static int big_b(int x) {
    int u0 = x - 4, u1 = x - 3, u2 = x - 2, u3 = x - 1;
    int u4 = x + 1, u5 = x + 2, u6 = x + 3, u7 = x + 4;
    sink = u0; sink = u1; sink = u2; sink = u3;
    sink = u4; sink = u5; sink = u6; sink = u7;
    return u0 + u1 + u2 + u3 + u4 + u5 + u6 + u7;
}
void main(void) {
    out = (unsigned char)(big_a(in) + big_b(in + 1));
}
