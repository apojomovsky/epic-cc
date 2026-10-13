/* epic-cc#1212: a constant search and a bounded runtime loop in
 * the same function. The search must fold; the runtime loop over
 * volatile data must stay rolled (it has no constant divide, so
 * the fold stage must not make it unrollable). */
typedef unsigned long u32;
typedef unsigned short u16;
typedef unsigned char u8;

volatile u8 out_pr2;
volatile u8 out_pre;
volatile u8 out_post;
volatile u8 arr[12];

static void tick_and_scale(u32 fosc_hz)
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
    out_pr2 = best_pr2;
    out_pre = best_pre;
    out_post = best_post;
    for (int i = 0; i < 12; i++) {
        arr[i] = (u8)(arr[i] * 3u + 1u);
    }
}

int main(void)
{
    tick_and_scale(20000000u);
    for (;;) {
    }
}
