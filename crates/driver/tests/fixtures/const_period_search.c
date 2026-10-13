/* epic-cc#1212: the epic_tick compute_period shape. A 3x16x2
 * prescaler/postscaler search over u32 arithmetic, called only with
 * the constant 20 MHz, so the whole search must fold to constant
 * stores: PR2 249, prescaler enum 1, postscaler 4 (target 5000 is
 * hit exactly by 4 * 5 * 250). Volatile outputs keep the result
 * observable to the test. */
typedef unsigned long u32;
typedef unsigned short u16;
typedef unsigned char u8;

volatile u8 out_pr2;
volatile u8 out_pre;
volatile u8 out_post;

static void compute_period(u32 fosc_hz, u8 *pr2, u8 *pre, u8 *post)
{
    u32 target = fosc_hz / 4000u;
    if (target == 0u) {
        target = 1u;
    }
    static const u8 pre_enum[3] = { 2u, 1u, 0u };
    static const u16 pre_ratio[3] = { 16u, 4u, 1u };
    u32 best_err = 0xFFFFFFFFu;
    u8 best_pr2 = 0xFFu;
    u8 best_pre = 0u;
    u8 best_post = 0u;
    for (int i = 0; i < 3; i++) {
        u32 p = pre_ratio[i];
        for (u32 q = 1u; q <= 16u; q++) {
            u32 pq = p * q;
            u32 n = target / pq;
            for (u32 k = 0u; k < 2u; k++) {
                u32 nn = n + k;
                if (nn < 1u || nn > 256u) {
                    continue;
                }
                u32 cand = pq * nn;
                u32 err = (cand >= target) ? (cand - target) : (target - cand);
                if (err < best_err) {
                    best_err = err;
                    best_pr2 = (u8)(nn - 1u);
                    best_pre = pre_enum[i];
                    best_post = (u8)(q - 1u);
                }
            }
        }
    }
    *pr2 = best_pr2;
    *pre = best_pre;
    *post = best_post;
}

void tick_init(u32 fosc_hz)
{
    u8 pr2, pre, post;
    compute_period(fosc_hz, &pr2, &pre, &post);
    out_pr2 = pr2;
    out_pre = pre;
    out_post = post;
}

int main(void)
{
    tick_init(20000000u);
    for (;;) {
    }
}
