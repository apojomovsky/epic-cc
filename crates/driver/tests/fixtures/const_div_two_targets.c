/* epic-cc#1212: two constant-divide functions. The search folds
 * and shrinks; the scaling loop divides by a constant but runs
 * over a volatile bound, so it cannot fold or shrink. The search
 * must still fold: a non-shrinking target must not veto it. */
typedef unsigned long u32;
typedef unsigned short u16;
typedef unsigned char u8;

volatile u8 out_pr2;
volatile u8 out_pre;
volatile u8 out_post;
volatile u8 arr[16];
volatile u8 n_arr;

static void search(u32 fosc_hz)
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
            u32 nn0 = target / pq;
            for (u32 k = 0u; k < 2u; k++) {
                u32 nn = nn0 + k;
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
}

static void scale(void)
{
    for (u8 i = 0u; i < n_arr; i++) {
        arr[i] = (u8)(arr[i] / 7u + 1u);
    }
}

int main(void)
{
    search(20000000u);
    scale();
    for (;;) {
    }
}
